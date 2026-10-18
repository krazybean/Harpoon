use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_TARGETS: usize = 32;
pub const MAX_COMMAND_BYTES: usize = 8 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_DURATION: Duration = Duration::from_secs(30);

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub container_id: String,
    pub context: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResultRow {
    pub container_id: String,
    pub status: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

pub fn validate_dispatch(command: &str, targets: &[Target]) -> Result<(), String> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_COMMAND_BYTES {
        return Err("command must be between 1 byte and 8 KiB".into());
    }
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(format!("select between 1 and {} containers", MAX_TARGETS));
    }
    for target in targets {
        if target.context != "harpoon" {
            return Err("universal commands require the harpoon Docker context".into());
        }
        if target.container_id.len() != 64
            || !target.container_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("universal commands require full 64-character container IDs".into());
        }
    }
    Ok(())
}

fn read_bounded<R: Read>(reader: R) -> String {
    let mut bytes = Vec::with_capacity(MAX_OUTPUT_BYTES.min(8192));
    BufReader::new(reader)
        .take((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok();
    let truncated = bytes.len() > MAX_OUTPUT_BYTES;
    bytes.truncate(MAX_OUTPUT_BYTES);
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[output truncated at 64 KiB]");
    }
    output
}

struct ChildOutput {
    status: Option<ExitStatus>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn prepare_child(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_bounded(mut child: Child, timeout: Duration) -> ChildOutput {
    let stdout = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(|| read_bounded(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(|| read_bounded(pipe)));
    let started = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if started.elapsed() >= timeout => {
                terminate_child(&mut child);
                break (None, true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                terminate_child(&mut child);
                break (None, false);
            }
        }
    };
    ChildOutput {
        status,
        stdout: stdout
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default(),
        stderr: stderr
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default(),
        timed_out,
    }
}

fn inspect_running(docker: &Path, target: &Target, timeout: Duration) -> Result<(), String> {
    let mut command = Command::new(docker);
    command
        .args([
            "--context",
            "harpoon",
            "inspect",
            "--format",
            "{{.Id}}\t{{.State.Running}}",
            &target.container_id,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_child(&mut command);
    let child = command
        .spawn()
        .map_err(|e| format!("container validation failed: {}", e))?;
    let output = run_bounded(child, timeout);
    if output.timed_out {
        return Err("container validation timed out".into());
    }
    if !output
        .status
        .map(|status| status.success())
        .unwrap_or(false)
    {
        return Err("container no longer exists in the harpoon context".into());
    }
    let line = output.stdout.trim();
    let mut fields = line.trim().split('\t');
    if fields.next() != Some(target.container_id.as_str()) || fields.next() != Some("true") {
        return Err("container identity or running state changed before dispatch".into());
    }
    Ok(())
}

fn execute_one(docker: &Path, target: Target, command: &str, timeout: Duration) -> ResultRow {
    if let Err(error) = inspect_running(docker, &target, timeout) {
        let status = if error == "container validation timed out" {
            "timeout"
        } else {
            "skipped"
        };
        return ResultRow {
            container_id: target.container_id,
            status: status.into(),
            output: String::new(),
            exit_code: None,
            error: Some(error),
        };
    }
    let mut child_command = Command::new(docker);
    child_command
        .args([
            "--context",
            "harpoon",
            "exec",
            &target.container_id,
            "/bin/sh",
            "-c",
            command.trim(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_child(&mut child_command);
    let child = match child_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ResultRow {
                container_id: target.container_id,
                status: "failed".into(),
                output: String::new(),
                exit_code: None,
                error: Some(error.to_string()),
            }
        }
    };
    let result = run_bounded(child, timeout);
    let mut output = [result.stdout, result.stderr]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("");
    if output.len() > MAX_OUTPUT_BYTES {
        output.truncate(MAX_OUTPUT_BYTES);
        output.push_str("\n[combined output truncated at 64 KiB]");
    }
    if result.timed_out {
        return ResultRow {
            container_id: target.container_id,
            status: "timeout".into(),
            output,
            exit_code: None,
            error: Some("command timed out".into()),
        };
    }
    let code = result.status.and_then(|status| status.code());
    ResultRow {
        container_id: target.container_id,
        status: if code == Some(0) {
            "success".into()
        } else {
            "failed".into()
        },
        output,
        exit_code: code,
        error: None,
    }
}

pub fn execute(
    docker: &Path,
    command: &str,
    targets: Vec<Target>,
) -> Result<Vec<ResultRow>, String> {
    execute_with_timeout(docker, command, targets, MAX_DURATION)
}

fn execute_with_timeout(
    docker: &Path,
    command: &str,
    targets: Vec<Target>,
    timeout: Duration,
) -> Result<Vec<ResultRow>, String> {
    validate_dispatch(command, &targets)?;
    let mut workers = Vec::with_capacity(targets.len());
    for target in targets {
        let docker = docker.to_path_buf();
        let command = command.to_owned();
        workers.push(thread::spawn(move || {
            execute_one(&docker, target, &command, timeout)
        }));
    }
    let mut results = Vec::with_capacity(workers.len());
    for worker in workers {
        results.push(worker.join().unwrap_or_else(|_| ResultRow {
            container_id: String::new(),
            status: "failed".into(),
            output: String::new(),
            exit_code: None,
            error: Some("command worker terminated unexpectedly".into()),
        }));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn target(id: &str) -> Target {
        Target {
            container_id: id.repeat(64 / id.len()),
            context: "harpoon".into(),
        }
    }

    #[cfg(unix)]
    fn fake_docker() -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "harpoon-broadcast-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(
            &path,
            r#"#!/bin/sh
if [ "$3" = "inspect" ]; then
  case "$6" in e*) sleep 2 ;; esac
  printf '%s\ttrue\n' "$6"
  exit 0
fi
if [ "$3" = "exec" ]; then
  case "$7" in
    fail) printf 'rejected\n' >&2; exit 7 ;;
    mix) case "$4" in b*) printf 'rejected\n' >&2; exit 7 ;; esac; printf 'ok\n' ;;
    slow) sleep 2 ;;
    huge) head -c 70000 /dev/zero | tr '\0' 'x' ;;
    *) printf '%s\n' "$7" ;;
  esac
fi
"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn dispatch_validates_scope_and_full_identity() {
        assert!(validate_dispatch("uptime", &[target("a")]).is_ok());
        assert!(validate_dispatch(" ", &[target("a")]).is_err());
        assert!(validate_dispatch(
            "uptime",
            &[Target {
                container_id: "abc".into(),
                context: "harpoon".into(),
            }]
        )
        .is_err());
        assert!(validate_dispatch(
            "uptime",
            &[Target {
                container_id: "a".repeat(64),
                context: "desktop".into(),
            }]
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn mocked_targets_finish_independently_with_timeout_and_bounded_output() {
        let docker = fake_docker();
        let targets = vec![target("a"), target("b"), target("c"), target("d")];
        let started = Instant::now();
        let results =
            execute_with_timeout(&docker, "slow", targets, Duration::from_millis(500)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(results.len(), 4);
        assert!(
            results.iter().all(|result| result.status == "timeout"),
            "{results:?}"
        );

        let results = execute_with_timeout(
            &docker,
            "fail",
            vec![target("a"), target("b")],
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(results
            .iter()
            .all(|result| result.status == "failed" && result.exit_code == Some(7)));

        let results = execute_with_timeout(
            &docker,
            "mix",
            vec![target("a"), target("b")],
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(results[0].status, "success");
        assert_eq!(results[1].status, "failed");

        let results =
            execute_with_timeout(&docker, "huge", vec![target("a")], Duration::from_secs(1))
                .unwrap();
        assert_eq!(results[0].status, "success");
        assert!(results[0].output.len() <= MAX_OUTPUT_BYTES + 40);
        fs::remove_file(docker).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn mocked_inspect_timeout_is_reported_instead_of_hanging() {
        let docker = fake_docker();
        let results = execute_with_timeout(
            &docker,
            "uptime",
            vec![target("e")],
            Duration::from_millis(500),
        )
        .unwrap();
        assert_eq!(results[0].status, "timeout");
        assert_eq!(
            results[0].error.as_deref(),
            Some("container validation timed out")
        );
        fs::remove_file(docker).unwrap();
    }
}
