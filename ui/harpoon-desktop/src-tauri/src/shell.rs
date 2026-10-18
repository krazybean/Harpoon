use serde::Serialize;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex};

const MAX_INPUT_BYTES: usize = 64 * 1024;
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;

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
    pid: libc::pid_t,
    master_fd: libc::c_int,
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
        // Kill only the process forked for this session. The PTY reader closes the fd
        // after the child exits, avoiding a cross-thread double-close race.
        unsafe {
            libc::kill(session.pid, libc::SIGTERM);
            libc::kill(session.pid, libc::SIGKILL);
        }
        Ok(())
    }

    pub fn close_all(&self) {
        let sessions = self
            .sessions
            .lock()
            .map(|mut sessions| sessions.drain().map(|(_, s)| s).collect::<Vec<_>>())
            .unwrap_or_default();
        for session in sessions {
            unsafe {
                libc::kill(session.pid, libc::SIGTERM);
                libc::kill(session.pid, libc::SIGKILL);
            }
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
    let cols = cols.unwrap_or(DEFAULT_COLS).clamp(1, 500);
    let rows = rows.unwrap_or(DEFAULT_ROWS).clamp(1, 200);
    let session_id = format!("shell-{}", uuid_like_id());
    let docker = docker.to_string_lossy().into_owned();
    let args = docker_args(&docker, &context, &container_id);
    let mut master_fd = -1;
    let window = pty_window(cols, rows);

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
        let argv: Vec<*const libc::c_char> = args
            .iter()
            .map(|arg| arg.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        unsafe {
            libc::execvp(args[0].as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }

    if let Ok(mut sessions) = manager.sessions.lock() {
        sessions.insert(session_id.clone(), Session { pid, master_fd });
    } else {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::close(master_fd);
        }
        return Err("shell session manager unavailable".to_string());
    }

    let reader_id = session_id.clone();
    let reader_manager = manager.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(format!("harpoon-{}", reader_id))
        .spawn(move || read_session(app, reader_manager, reader_id, pid, master_fd))
    {
        let _ = manager.close(&session_id);
        return Err(format!("failed to start shell reader: {}", error));
    }

    Ok(ShellSession {
        session_id,
        context,
        container_id,
    })
}

fn read_session(
    app: tauri::AppHandle,
    manager: Arc<ShellManager>,
    session_id: String,
    pid: libc::pid_t,
    fd: libc::c_int,
) {
    let mut buf = [0_u8; OUTPUT_CHUNK_BYTES];
    loop {
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read <= 0 {
            break;
        }
        emit(
            &app,
            ShellEvent {
                session_id: session_id.clone(),
                kind: "output",
                data: Some(String::from_utf8_lossy(&buf[..read as usize]).into_owned()),
                code: None,
                error: None,
            },
        );
    }
    let mut status = 0;
    unsafe {
        libc::waitpid(pid, &mut status, 0);
        libc::close(fd);
    }
    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        -1
    };
    let _ = manager.remove(&session_id);
    if code == 127 {
        emit(
            &app,
            ShellEvent {
                session_id: session_id.clone(),
                kind: "error",
                data: None,
                code: Some(code),
                error: Some("Docker CLI could not be executed".to_string()),
            },
        );
    }
    emit(
        &app,
        ShellEvent {
            session_id,
            kind: "exit",
            data: None,
            code: Some(code),
            error: None,
        },
    );
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
}
