use serde::Serialize;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};

const MAX_INPUT_BYTES: usize = 64 * 1024;
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(750);
const REAP_POLL: std::time::Duration = std::time::Duration::from_millis(20);

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellEvent {
    pub session_id: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellSession {
    pub session_id: String,
    pub context: String,
    pub container_id: String,
}

struct Session {
    child: Arc<ChildPid>,
    master_fd: libc::c_int,
}

/// The forked Docker CLI's pid, cleared by the reader once it has been reaped.
/// Signals are only sent while holding this lock and before the reap, so a
/// recycled pid can never be signalled.
struct ChildPid(Mutex<Option<libc::pid_t>>);

impl ChildPid {
    fn signal(&self, signal: libc::c_int) {
        if let Ok(pid) = self.0.lock() {
            if let Some(pid) = *pid {
                unsafe {
                    libc::kill(pid, signal);
                }
            }
        }
    }

    /// Polls until the child exits and returns its raw wait status.
    fn reap(&self) -> libc::c_int {
        loop {
            if let Ok(mut pid) = self.0.lock() {
                let Some(child) = *pid else { return 0 };
                let mut status = 0;
                let reaped = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
                if reaped == child || reaped < 0 {
                    *pid = None;
                    return status;
                }
            } else {
                return 0;
            }
            std::thread::sleep(REAP_POLL);
        }
    }
}

pub struct ShellManager {
    sessions: Mutex<HashMap<String, Session>>,
}

impl ShellManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn remove(&self, session_id: &str) -> Option<Session> {
        self.sessions.lock().ok()?.remove(session_id)
    }

    pub fn close(&self, session_id: &str) -> Result<(), String> {
        let session = self
            .remove(session_id)
            .ok_or_else(|| "shell session not found".to_string())?;
        // Kill only the process forked for this session. The PTY reader reaps it and
        // closes the fd, avoiding a cross-thread double-close race. Give the Docker
        // CLI a moment to detach cleanly before forcing it.
        session.child.signal(libc::SIGTERM);
        let child = session.child.clone();
        let _ = std::thread::Builder::new()
            .name("harpoon-shell-close".into())
            .spawn(move || {
                std::thread::sleep(CLOSE_GRACE);
                child.signal(libc::SIGKILL);
            });
        Ok(())
    }

    pub fn close_all(&self) {
        let sessions = self
            .sessions
            .lock()
            .map(|mut sessions| sessions.drain().map(|(_, s)| s).collect::<Vec<_>>())
            .unwrap_or_default();
        for session in sessions {
            session.child.signal(libc::SIGKILL);
        }
    }
}

impl Drop for ShellManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

pub fn validate_container_id(id: &str) -> Result<(), String> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("shell requires the full 64-character container ID".to_string());
    }
    Ok(())
}

fn validate_context(context: &str) -> Result<(), String> {
    if context.is_empty()
        || context.len() > 128
        || !context
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err("invalid Docker context".to_string());
    }
    Ok(())
}

fn docker_args(docker: &str, context: &str, container_id: &str) -> Vec<CString> {
    [
        docker,
        "--context",
        context,
        "exec",
        "-it",
        container_id,
        "/bin/sh",
    ]
    .into_iter()
    .map(|arg| CString::new(arg).expect("validated shell argument has no NUL"))
    .collect()
}

fn pty_window(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows.clamp(1, 200),
        ws_col: cols.clamp(1, 500),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Decodes PTY output without splitting multi-byte characters across reads.
/// Incomplete trailing bytes stay in `pending` for the next chunk; invalid bytes
/// still become U+FFFD.
fn decode_output(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let mut text = String::new();
    let mut rest: &[u8] = pending;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                rest = &[];
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                text.push_str(std::str::from_utf8(valid).expect("prefix is valid UTF-8"));
                match error.error_len() {
                    Some(len) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        rest = &after[len..];
                    }
                    None => {
                        rest = after;
                        break;
                    }
                }
            }
        }
    }
    *pending = rest.to_vec();
    text
}

fn emit(app: &tauri::AppHandle, event: ShellEvent) {
    let _ = tauri::Emitter::emit(app, "shell-event", event);
}

pub fn open(
    app: tauri::AppHandle,
    manager: Arc<ShellManager>,
    context: String,
    container_id: String,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<ShellSession, String> {
    validate_context(&context)?;
    validate_container_id(&container_id)?;
    let docker = crate::resolve_docker_binary()?;
    let docker = docker.to_string_lossy().into_owned();
    let args = docker_args(&docker, &context, &container_id);
    let session_id = spawn_session(manager, args, cols, rows, move |event| emit(&app, event))?;
    Ok(ShellSession {
        session_id,
        context,
        container_id,
    })
}

fn spawn_session(
    manager: Arc<ShellManager>,
    args: Vec<CString>,
    cols: Option<u16>,
    rows: Option<u16>,
    emit: impl Fn(ShellEvent) + Send + 'static,
) -> Result<String, String> {
    let cols = cols.unwrap_or(DEFAULT_COLS).clamp(1, 500);
    let rows = rows.unwrap_or(DEFAULT_ROWS).clamp(1, 200);
    let session_id = format!("shell-{}", uuid_like_id());
    let mut master_fd = -1;
    let window = pty_window(cols, rows);
    // Everything the child touches is allocated before fork: allocating after
    // fork in a multithreaded process can deadlock on the allocator lock.
    let argv: Vec<*const libc::c_char> = args
        .iter()
        .map(|arg| arg.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    const EXEC_FAILED: &[u8] = b"harpoon: the Docker CLI could not be executed\r\n";

    let pid = unsafe {
        libc::forkpty(
            &mut master_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &window as *const _ as *mut _,
        )
    };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        unsafe {
            libc::execvp(argv[0], argv.as_ptr());
            libc::write(2, EXEC_FAILED.as_ptr().cast(), EXEC_FAILED.len());
            libc::_exit(127);
        }
    }
    // Keep this session's PTY master out of every later child process.
    unsafe {
        libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }

    let child = Arc::new(ChildPid(Mutex::new(Some(pid))));
    if let Ok(mut sessions) = manager.sessions.lock() {
        sessions.insert(
            session_id.clone(),
            Session {
                child: child.clone(),
                master_fd,
            },
        );
    } else {
        child.signal(libc::SIGKILL);
        child.reap();
        unsafe {
            libc::close(master_fd);
        }
        return Err("shell session manager unavailable".to_string());
    }

    let reader_id = session_id.clone();
    let reader_manager = manager.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(format!("harpoon-{}", reader_id))
        .spawn(move || read_session(emit, reader_manager, reader_id, child, master_fd))
    {
        if let Some(session) = manager.remove(&session_id) {
            session.child.signal(libc::SIGKILL);
            session.child.reap();
            unsafe {
                libc::close(session.master_fd);
            }
        }
        return Err(format!("failed to start shell reader: {}", error));
    }

    Ok(session_id)
}

fn read_session(
    emit: impl Fn(ShellEvent),
    manager: Arc<ShellManager>,
    session_id: String,
    child: Arc<ChildPid>,
    fd: libc::c_int,
) {
    let mut buf = [0_u8; OUTPUT_CHUNK_BYTES];
    let mut pending = Vec::new();
    loop {
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if read <= 0 {
            break;
        }
        let data = decode_output(&mut pending, &buf[..read as usize]);
        if data.is_empty() {
            continue;
        }
        emit(ShellEvent {
            session_id: session_id.clone(),
            kind: "output",
            data: Some(data),
            code: None,
            error: None,
        });
    }
    if !pending.is_empty() {
        emit(ShellEvent {
            session_id: session_id.clone(),
            kind: "output",
            data: Some(String::from_utf8_lossy(&pending).into_owned()),
            code: None,
            error: None,
        });
    }
    // Unregister before closing: once the fd is closed its number can be reused,
    // and write()/resize() must never reach it through a stale session.
    let _ = manager.remove(&session_id);
    let status = child.reap();
    unsafe {
        libc::close(fd);
    }
    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        -1
    };
    emit(ShellEvent {
        session_id,
        kind: "exit",
        data: None,
        code: Some(code),
        error: None,
    });
}

pub fn write(manager: &ShellManager, session_id: String, input: String) -> Result<(), String> {
    if input.len() > MAX_INPUT_BYTES {
        return Err("shell input exceeds 64 KiB".to_string());
    }
    let session = manager
        .sessions
        .lock()
        .map_err(|_| "shell session manager unavailable".to_string())?;
    let session = session
        .get(&session_id)
        .ok_or_else(|| "shell session not found".to_string())?;
    let bytes = input.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        let written = unsafe {
            libc::write(
                session.master_fd,
                bytes[offset..].as_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if written < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if written <= 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        offset += written as usize;
    }
    Ok(())
}

pub fn resize(
    manager: &ShellManager,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = manager
        .sessions
        .lock()
        .map_err(|_| "shell session manager unavailable".to_string())?;
    let session = session
        .get(&session_id)
        .ok_or_else(|| "shell session not found".to_string())?;
    let window = pty_window(cols, rows);
    let result = unsafe { libc::ioctl(session.master_fd, libc::TIOCSWINSZ, &window) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_full_ids_and_safe_contexts() {
        assert!(validate_container_id(&"a".repeat(64)).is_ok());
        assert!(validate_container_id("container;id").is_err());
        assert!(validate_context("harpoon").is_ok());
        assert!(validate_context("harpoon; rm -rf /").is_err());
    }

    #[test]
    fn frames_docker_exec_without_a_shell() {
        let args = docker_args("/usr/bin/docker", "harpoon", &"b".repeat(64));
        let args: Vec<_> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(
            args,
            vec![
                "/usr/bin/docker",
                "--context",
                "harpoon",
                "exec",
                "-it",
                &"b".repeat(64),
                "/bin/sh"
            ]
        );
    }

    #[test]
    fn maps_terminal_columns_and_rows_to_the_matching_pty_axes() {
        let window = pty_window(123, 45);
        assert_eq!(window.ws_col, 123);
        assert_eq!(window.ws_row, 45);
    }
    #[test]
    fn multibyte_characters_split_across_reads_decode_intact() {
        let text = "中文 ✓";
        let bytes = text.as_bytes();
        let mut pending = Vec::new();
        let mut decoded = String::new();
        for chunk in bytes.chunks(2) {
            decoded.push_str(&decode_output(&mut pending, chunk));
        }
        assert_eq!(decoded, text);
        assert!(pending.is_empty());
        assert_eq!(decode_output(&mut pending, b"a\xffb"), "a\u{fffd}b");
    }

    fn collect(
        argv: &[&str],
    ) -> (
        Arc<ShellManager>,
        String,
        std::sync::mpsc::Receiver<ShellEvent>,
    ) {
        let manager = Arc::new(ShellManager::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let args = argv.iter().map(|arg| CString::new(*arg).unwrap()).collect();
        let id = spawn_session(manager.clone(), args, None, None, move |event| {
            let _ = tx.lock().unwrap().send(event);
        })
        .unwrap();
        (manager, id, rx)
    }

    fn wait_for_exit(rx: &std::sync::mpsc::Receiver<ShellEvent>) -> (String, i32) {
        let mut output = String::new();
        loop {
            let event = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("shell session should exit");
            match event.kind {
                "output" => output.push_str(event.data.as_deref().unwrap_or("")),
                "exit" => return (output, event.code.unwrap()),
                _ => {}
            }
        }
    }

    #[test]
    fn session_streams_output_and_unregisters_before_exit_is_published() {
        let (manager, id, rx) = collect(&["/bin/sh", "-c", "printf '中文%.0s' 1 2 3; exit 3"]);
        let (output, code) = wait_for_exit(&rx);
        assert!(output.contains("中文中文中文"), "{output:?}");
        assert_eq!(code, 3);
        assert_eq!(
            write(&manager, id, "late".into()).unwrap_err(),
            "shell session not found"
        );
    }

    #[test]
    fn exec_failure_is_explained_in_the_terminal() {
        let (_manager, _id, rx) = collect(&["/nonexistent/harpoon-docker"]);
        let (output, code) = wait_for_exit(&rx);
        assert!(output.contains("Docker CLI could not be executed"), "{output:?}");
        assert_eq!(code, 127);
    }

    #[test]
    fn close_escalates_when_the_child_ignores_sigterm() {
        let (manager, id, rx) = collect(&["/bin/sh", "-c", "trap '' TERM; echo ready; sleep 30"]);
        let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(first.kind, "output");
        let started = std::time::Instant::now();
        manager.close(&id).unwrap();
        let (_, code) = wait_for_exit(&rx);
        assert_eq!(code, 128 + libc::SIGKILL);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
