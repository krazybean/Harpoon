#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

mod shell;
mod broadcast;
mod runtime_refresh;
mod resource_sampler;
mod context_discovery;
mod bootstrap;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct HarpoonStatus {
    state: String,
    #[serde(default)]
    pid: Option<u64>,
    #[serde(default)]
    cpus: Option<u32>,
    #[serde(default, rename = "memoryMiB")]
    memory_mib: Option<u32>,
    #[serde(default, rename = "diskPath")]
    disk_path: Option<String>,
    #[serde(default, rename = "diskLogicalBytes")]
    disk_logical_bytes: Option<u64>,
    #[serde(default, rename = "socketPath")]
    socket_path: Option<String>,
    #[serde(default, rename = "sockExists")]
    sock_exists: Option<bool>,
    #[serde(default, rename = "lockHeld")]
    lock_held: Option<bool>,
    #[serde(default, rename = "lockPath")]
    lock_path: Option<String>,
    #[serde(default, rename = "logPath")]
    log_path: Option<String>,
    #[serde(default, rename = "dockerReady")]
    docker_ready: Option<bool>,
    #[serde(default)]
    failure: Option<RuntimeFailure>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct RuntimeFailure {
    category: String,
    message: String,
    source: String,
    #[serde(rename = "occurredAt")]
    occurred_at: String,
    #[serde(rename = "underlyingError")]
    underlying_error: Option<String>,
}

#[derive(Serialize, Debug)]
struct DoctorResult {
    raw: String,
    passed: u32,
    warnings: u32,
    failures: u32,
}

#[derive(Serialize, Debug)]
struct ConfigResult {
    cpus: u32,
    memory: u32,
    raw: String,
    path: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ContainerStats {
    source: &'static str,
    container_id: String,
    cpu_percent: Option<String>,
    memory_usage: Option<String>,
    memory_percent: Option<String>,
    network_io: Option<String>,
    block_io: Option<String>,
    pids: Option<String>,
}

// --- Harpoon binary resolution with correct precedence ---
fn is_executable(p: &PathBuf) -> bool {
    if !p.exists() { return false; }
    if p.is_dir() { return false; }
    // On Unix, check executable bit via metadata
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(p) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    true
}

fn bundled_harpoon_path() -> Option<PathBuf> {
    // Harpoon.app Resources bundling (production) + dev seam.
    // Preferred layout: Harpoon.app/Contents/Resources/harpoon/bin/harpoon
    //   with lib assets at Harpoon.app/Contents/Resources/harpoon/lib/harpoon/*
    // Fallbacks for flat Resources (Tauri default) and dev checkout.
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidates = [
        // Production Harpoon.app structured layout (current Tauri bundle)
        exe_dir.join("../Resources/bundle-resources/harpoon/bin/harpoon"),
        exe_dir.join("../Resources/harpoon/bin/harpoon"),
        exe_dir.join("../Resources/harpoon/lib/harpoon/harpoon"),
        // Flat Resources (Tauri copies file basename to Resources)
        exe_dir.join("../Resources/harpoon"),
        exe_dir.join("../Resources/harpoon-initramfs.cpio.gz").with_file_name("harpoon"),
        // Alternate relative
        exe_dir.join("../../Resources/harpoon/bin/harpoon"),
        exe_dir.join("../../Resources/bundle-resources/harpoon/bin/harpoon"),
        exe_dir.join("../../Resources/harpoon"),
        // Development seam
        exe_dir.join("../Resources/harpoon/build/harpoon"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../Resources/harpoon"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../Resources/harpoon/build/harpoon"),
    ];
    for c in &candidates {
        if is_executable(c) {
            // canonicalize may fail if symlink loop, so fallback to original
            return Some(c.clone());
        }
        // also try canonicalized
        if let Ok(canonical) = c.canonicalize() {
            if is_executable(&canonical) {
                return Some(canonical);
            }
        }
    }
    None
}

fn resolve_harpoon_binary() -> Result<PathBuf, String> {
    // 1. HARPOON_BIN env var if present and executable
    if let Ok(env_path) = std::env::var("HARPOON_BIN") {
        let p = PathBuf::from(env_path.trim());
        if is_executable(&p) {
            return Ok(p);
        }
        // Also try canonicalize
        if let Ok(c) = p.canonicalize() {
            if is_executable(&c) { return Ok(c); }
        }
        // Env var present but not executable -> error with detail, don't silently fall through to wrong binary
        return Err(format!("HARPOON_BIN points to non-executable '{}'", p.display()));
    }
    // Development builds must use the checkout they were compiled from, not an
    // installed Harpoon.app that happens to be discoverable on this machine.
    //    src-tauri lives at <repo>/ui/harpoon-desktop/src-tauri
    //    development binary is <repo>/harpoon/build/harpoon
    let dev_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../harpoon/build/harpoon");
    if cfg!(debug_assertions) {
        if is_executable(&dev_bin) {
            if let Ok(c) = dev_bin.canonicalize() { return Ok(c); }
            return Ok(dev_bin);
        }
        if let Ok(c) = dev_bin.canonicalize() {
            if c.exists() { return Ok(c); }
        }
        if dev_bin.exists() { return Ok(dev_bin); }
    }
    // Bundled application resource seam (preferred for production Harpoon.app)
    if let Some(bundled) = bundled_harpoon_path() {
        return Ok(bundled);
    }
    // Installed fallbacks for non-bundled release environments.
    let p4 = PathBuf::from("/usr/local/bin/harpoon");
    if is_executable(&p4) { return Ok(p4); }
    // 5. /opt/homebrew/bin/harpoon
    let p5 = PathBuf::from("/opt/homebrew/bin/harpoon");
    if is_executable(&p5) { return Ok(p5); }
    // 6. PATH fallback — manual search, no shell
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() { continue; }
            let candidate = PathBuf::from(dir).join("harpoon");
            if is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err("harpoon binary not found (checked HARPOON_BIN, bundled Resources harpoon/bin/harpoon, <repo>/harpoon/build/harpoon via CARGO_MANIFEST_DIR, /usr/local/bin/harpoon, /opt/homebrew/bin/harpoon, PATH)".to_string())
}

// --- Canonical Docker CLI discovery (Finder-safe) ---
fn resolve_docker_binary() -> Result<PathBuf, String> {
    // Every docker call goes through here, so the probe result is cached; a cached
    // path is re-resolved only once it stops being executable.
    static RESOLVED: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    let cache = RESOLVED.get_or_init(|| Mutex::new(None));
    if let Ok(cached) = cache.lock() {
        if let Some(path) = cached.as_ref().filter(|path| is_executable(path)) {
            return Ok(path.clone());
        }
    }
    let path = find_docker_binary()?;
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(path.clone());
    }
    Ok(path)
}

fn find_docker_binary() -> Result<PathBuf, String> {
    // ponytail: explicit, no `zsh -l -c`, no sourcing startup files.
    // Finder-launched apps have minimal PATH (e.g. /usr/bin:/bin:/usr/sbin:/sbin).
    // We safely consider PATH first, then Apple Silicon Homebrew, the
    // /usr/local/bin symlink that Docker Desktop and OrbStack install, and other
    // standard locations, and finally the CLI bundled in Harpoon.app. Every
    // candidate must launch successfully, which also rejects stale binaries.
    let candidates = [
        PathBuf::from("/opt/homebrew/bin/docker"),
        PathBuf::from("/usr/local/bin/docker"),
        PathBuf::from("/usr/bin/docker"),
        PathBuf::from("/run/current-system/sw/bin/docker"),
    ];
    // 1) Try PATH first (process inherited)
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() { continue; }
            let p = PathBuf::from(dir).join("docker");
            if is_runnable_docker(&p) { return Ok(p); }
        }
    }
    // 2) Check standard locations regardless of PATH (Finder fallback)
    for p in &candidates {
        if is_runnable_docker(p) { return Ok(p.clone()); }
    }
    // 3) The Docker CLI bundled beside the runtime, for Macs without their own.
    if let Some(bundled) = bundled_harpoon_path().map(|harpoon| harpoon.with_file_name("docker")) {
        if is_runnable_docker(&bundled) { return Ok(bundled); }
    }
    Err("Docker CLI not installed/found (checked runnable PATH entries, /opt/homebrew/bin/docker, /usr/local/bin/docker, /usr/bin/docker, bundled Resources/harpoon/bin/docker)".to_string())
}

fn is_runnable_docker(path: &PathBuf) -> bool {
    if !is_executable(path) { return false; }
    let mut command = Command::new(path);
    command.arg("--version").stdin(Stdio::null());
    run_command_bounded(&mut command, DOCKER_PROBE_TIMEOUT)
        .map(|(_, _, code)| code == 0)
        .unwrap_or(false)
}

const DOCKER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn docker_command(args: &[&str]) -> Result<Command, String> {
    let bin = resolve_docker_binary()?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    Ok(cmd)
}

fn run_harpoon(args: &[&str]) -> Result<(String, String, i32), String> {
    let bin = resolve_harpoon_binary()?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    let output = cmd.output().map_err(|e| format!("failed to execute harpoon {}: {}", args.join(" "), e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

/// Tauri runs commands that are not `async` on the main thread, so anything that
/// waits on a subprocess must not be one: it would freeze the window.
async fn off_main<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("background task failed: {error}"))?
}

#[tauri::command]
fn get_harpoon_binary_path() -> Result<String, String> {
    let p = resolve_harpoon_binary()?;
    Ok(p.display().to_string())
}

#[tauri::command(async)]
fn get_status() -> Result<HarpoonStatus, String> {
    let (stdout, stderr, code) = run_harpoon(&["status", "--json"])?;
    if code != 0 && stdout.trim().is_empty() {
        return Err(format!("harpoon status --json failed (code {}): {}", code, stderr));
    }
    let json_str = if stdout.trim().is_empty() { stderr } else { stdout };
    serde_json::from_str(&json_str).map_err(|e| format!("malformed status JSON: {} | raw: {}", e, json_str))
}

fn start_harpoon_blocking() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["start"])?;
    let combined = format!("{}{}", stdout, stderr);
    if combined.contains("VZErrorDomain") || combined.contains("HOST_VZ_START_FAILURE") {
        return Err(format!("HOST_VZ_START_FAILURE: {}", combined.trim()));
    }
    if combined.to_lowercase().contains("already running") {
        return Ok(combined.trim().to_string());
    }
    if code != 0 {
        return Err(combined.trim().to_string());
    }
    Ok(combined.trim().to_string())
}

fn stop_harpoon_blocking() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["stop"])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 && !combined.to_lowercase().contains("already stopped") && !combined.to_lowercase().contains("not running") {
        return Err(combined.trim().to_string());
    }
    Ok(combined.trim().to_string())
}

fn restart_harpoon_blocking() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["restart"])?;
    let combined = format!("{}{}", stdout, stderr);
    if combined.contains("VZErrorDomain") || combined.contains("HOST_VZ_START_FAILURE") {
        return Err(format!("HOST_VZ_START_FAILURE: {}", combined.trim()));
    }
    if code != 0 {
        return Err(combined.trim().to_string());
    }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn get_doctor() -> Result<DoctorResult, String> {
    let (stdout, _stderr, _code) = run_harpoon(&["doctor"])?;
    let raw = stdout.clone();
    let passed = raw.matches("PASS").count() as u32;
    let mut warnings = 0;
    let mut failures = 0;
    for line in raw.lines() {
        if line.contains("warnings") && line.contains("failures") {
            let parts: Vec<&str> = line.split(',').collect();
            for p in parts {
                let p = p.trim();
                if p.contains("warnings") {
                    warnings = p.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                if p.contains("failures") {
                    failures = p.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
            }
        }
    }
    if warnings == 0 && failures == 0 {
        warnings = raw.matches("WARN").count() as u32;
        failures = raw.matches("FAIL").count() as u32;
    }
    Ok(DoctorResult { raw, passed, warnings, failures })
}

#[tauri::command(async)]
fn get_log_path() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["logs", "--path"])?;
    let combined = format!("{}{}", stdout, stderr);
    let path = combined.lines().next().unwrap_or("").trim().to_string();
    if path.is_empty() {
        return Err(format!("logs --path failed (code {}): {}", code, combined));
    }
    Ok(path)
}

#[tauri::command(async)]
fn get_recent_logs(lines: Option<u32>) -> Result<String, String> {
    let n = lines.unwrap_or(200).min(500);
    let (stdout, _stderr, _code) = run_harpoon(&["logs", "--lines", &n.to_string()])?;
    let mut s = stdout;
    if s.len() > 65536 {
        s = s[s.len() - 65536..].to_string();
    }
    Ok(s)
}

#[tauri::command(async)]
fn get_config() -> Result<ConfigResult, String> {
    let (stdout, stderr, code) = run_harpoon(&["config", "show"])?;
    let raw = format!("{}{}", stdout, stderr);
    if code != 0 && !raw.contains("cpus") {
        return Err(raw);
    }
    let mut cpus = 2;
    let mut memory = 4096;
    for line in raw.lines() {
        let l = line.trim().to_lowercase();
        if l.starts_with("cpus:") {
            if let Some(v) = l.split(':').nth(1) {
                if let Ok(n) = v.trim().parse() { cpus = n; }
            }
        }
        if l.starts_with("memory:") {
            if let Some(v) = l.split(':').nth(1) {
                if let Ok(n) = v.trim().parse() { memory = n; }
            }
        }
    }
    let path = "/tmp/harpoon-runtime/config.json".to_string();
    Ok(ConfigResult { cpus, memory, raw, path })
}

#[tauri::command(async)]
fn set_memory(memory: u32) -> Result<String, String> {
    if memory < 512 { return Err(format!("memory must be at least 512 MiB, got {}", memory)); }
    let (stdout, stderr, code) = run_harpoon(&["config", "set", "memory", &memory.to_string()])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined); }
    Ok(combined)
}

#[tauri::command(async)]
fn set_cpus(cpus: u32) -> Result<String, String> {
    let allowed = [1, 2, 4];
    if !allowed.contains(&cpus) { return Err(format!("unsupported cpus {}, allowed {:?}", cpus, allowed)); }
    let (stdout, stderr, code) = run_harpoon(&["config", "set", "cpus", &cpus.to_string()])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined); }
    Ok(combined)
}

#[tauri::command(async)]
fn get_docker_info() -> Result<String, String> {
    let mut cmd = docker_command(&["--context", "harpoon", "info", "--format", "{{.ServerVersion}}"]).map_err(|e| format!("Docker CLI: FAIL — {}", e))?;
    let output = cmd.output().map_err(|e| format!("Docker CLI: FAIL — Docker CLI not installed/found ({})", e))?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(err);
    }
    Ok(s)
}

// --- Docker resource helpers (explicit harpoon context, --format json, no Desktop fallback) ---
fn run_docker(args: &[&str]) -> Result<(String, String, i32), String> {
    let mut cmd = docker_command(args).map_err(|e| format!("Docker CLI: FAIL — {}", e))?;
    // Always require harpoon context explicitly; caller must include --context harpoon
    let output = cmd.output().map_err(|e| format!("failed to execute docker {}: {}", args.join(" "), e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

fn run_docker_bounded(args: &[&str], timeout: Duration) -> Result<(String, String, i32), String> {
    let mut cmd = docker_command(args).map_err(|e| format!("Docker CLI: FAIL — {}", e))?;
    run_command_bounded(&mut cmd, timeout).map_err(|error| format!("docker {}: {}", args.join(" "), error))
}

fn run_command_bounded(cmd: &mut Command, timeout: Duration) -> Result<(String, String, i32), String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("failed to execute bounded command: {}", e))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().map_err(|e| format!("failed to read docker output: {e}"))?;
                return Ok((
                    String::from_utf8_lossy(&output.stdout).to_string(),
                    String::from_utf8_lossy(&output.stderr).to_string(),
                    output.status.code().unwrap_or(-1),
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} ms", timeout.as_millis()));
            }
            Err(e) => return Err(format!("failed waiting for docker: {e}")),
        }
    }
}

fn docker_json_lines(stdout: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            out.push(v);
        }
    }
    out
}

fn parse_compose_label(labels: &str, key: &str) -> Option<String> {
    for part in labels.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == key {
                let v = v.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                } else {
                    return None;
                }
            }
        }
    }
    None
}

fn enrich_compose_fields(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        // Clone Labels to avoid borrow conflict with subsequent inserts
        let labels_cloned = obj.get("Labels").cloned();
        if let Some(labels_val) = labels_cloned {
            if let Some(labels_str) = labels_val.as_str() {
                if let Some(project) = parse_compose_label(labels_str, "com.docker.compose.project") {
                    obj.insert("ComposeProject".to_string(), serde_json::Value::String(project));
                }
                if let Some(service) = parse_compose_label(labels_str, "com.docker.compose.service") {
                    obj.insert("ComposeService".to_string(), serde_json::Value::String(service));
                }
                if let Some(number) = parse_compose_label(labels_str, "com.docker.compose.container-number") {
                    obj.insert("ComposeNumber".to_string(), serde_json::Value::String(number));
                }
            } else if let Some(map) = labels_val.as_object() {
                // Fallback for API map form (e.g., /containers/json)
                if let Some(v) = map.get("com.docker.compose.project").and_then(|x| x.as_str()) {
                    let v = v.trim();
                    if !v.is_empty() { obj.insert("ComposeProject".to_string(), serde_json::Value::String(v.to_string())); }
                }
                if let Some(v) = map.get("com.docker.compose.service").and_then(|x| x.as_str()) {
                    let v = v.trim();
                    if !v.is_empty() { obj.insert("ComposeService".to_string(), serde_json::Value::String(v.to_string())); }
                }
                if let Some(v) = map.get("com.docker.compose.container-number").and_then(|x| x.as_str()) {
                    let v = v.trim();
                    if !v.is_empty() { obj.insert("ComposeNumber".to_string(), serde_json::Value::String(v.to_string())); }
                }
            }
        }
    }
}

fn enrich_container_attachments(value: &mut serde_json::Value, inspect: &serde_json::Value) {
    let Some(obj) = value.as_object_mut() else { return; };
    let Some(details) = inspect.as_object() else { return; };

    let networks = details
        .get("NetworkSettings")
        .and_then(|v| v.get("Networks"))
        .and_then(|v| v.as_object())
        .map(|map| map.keys().cloned().map(serde_json::Value::String).collect())
        .unwrap_or_else(|| Vec::new());
    obj.insert("NetworkNames".to_string(), serde_json::Value::Array(networks));
    let attachments = details
        .get("NetworkSettings")
        .and_then(|v| v.get("Networks"))
        .and_then(|v| v.as_object())
        .map(|map| map.iter().map(|(name, network)| serde_json::json!({
            "name": name,
            "endpointId": network.get("EndpointID").and_then(|v| v.as_str()),
            "ipAddress": network.get("IPAddress").and_then(|v| v.as_str()),
            "gateway": network.get("Gateway").and_then(|v| v.as_str()),
        })).collect())
        .unwrap_or_else(|| Vec::new());
    obj.insert("NetworkAttachments".to_string(), serde_json::Value::Array(attachments));

    let mut named_volumes = Vec::new();
    let mut bind_mounts = Vec::new();
    if let Some(mounts) = details.get("Mounts").and_then(|v| v.as_array()) {
        for mount in mounts {
            let mount_type = mount.get("Type").and_then(|v| v.as_str()).unwrap_or("");
            let target = mount.get("Destination").and_then(|v| v.as_str()).unwrap_or("");
            let read_only = mount.get("RW").and_then(|v| v.as_bool()).map(|rw| !rw).unwrap_or(false);
            match mount_type {
                "volume" => {
                    if let Some(name) = mount.get("Name").and_then(|v| v.as_str()) {
                        named_volumes.push(serde_json::json!({ "name": name, "target": target, "readOnly": read_only }));
                    }
                }
                "bind" => {
                    if let Some(source) = mount.get("Source").and_then(|v| v.as_str()) {
                        bind_mounts.push(serde_json::json!({ "source": source, "target": target, "readOnly": read_only }));
                    }
                }
                _ => {}
            }
        }
    }
    obj.insert("NamedVolumes".to_string(), serde_json::Value::Array(named_volumes));
    obj.insert("BindMounts".to_string(), serde_json::Value::Array(bind_mounts));
    obj.insert("GroupKind".to_string(), if obj.get("ComposeProject").is_some() {
        serde_json::Value::String("compose".to_string())
    } else {
        serde_json::Value::String("standalone".to_string())
    });
    obj.insert("GroupProvenance".to_string(), if obj.get("ComposeProject").is_some() {
        serde_json::Value::String("docker.labels".to_string())
    } else {
        serde_json::Value::String("docker.engine".to_string())
    });
}

/// Inspects containers for enrichment. `docker inspect a b` exits 1 when any one
/// container vanished since it was listed but still prints the rest, so partial
/// output is used rather than dropping enrichment for every row.
fn inspect_containers(ids: &[&str]) -> Vec<serde_json::Value> {
    if ids.is_empty() { return Vec::new(); }
    let mut args = vec!["--context", "harpoon", "inspect", "--format", "{{json .}}"];
    args.extend(ids.iter().copied());
    run_docker(&args).map(|(stdout, _, _)| docker_json_lines(&stdout)).unwrap_or_default()
}

fn attach_inspected(containers: &mut [serde_json::Value], inspected: &[serde_json::Value]) {
    for container in containers {
        let id = container.get("ID").or_else(|| container.get("Id")).and_then(|v| v.as_str());
        if let Some(details) = id.and_then(|wanted| inspected.iter().find(|item| item.get("Id").and_then(|v| v.as_str()).map(|actual| actual == wanted || actual.starts_with(wanted)).unwrap_or(false))) {
            enrich_container_attachments(container, details);
        }
    }
}

fn list_containers_basic(all: bool) -> Result<Vec<serde_json::Value>, String> {
    let mut args = vec!["--context", "harpoon", "ps"];
    if all { args.push("-a"); }
    args.extend(["--format", "{{json .}}", "--no-trunc"]);
    let (stdout, stderr, code) = run_docker(&args)?;
    if code != 0 && !stdout.contains("{") {
        return Err(stderr.trim().to_string());
    }
    let mut containers = docker_json_lines(&stdout);
    for c in &mut containers {
        enrich_compose_fields(c);
    }
    Ok(containers)
}

#[tauri::command(async)]
fn list_containers(all: Option<bool>) -> Result<Vec<serde_json::Value>, String> {
    let mut containers = list_containers_basic(all.unwrap_or(true))?;
    let ids: Vec<&str> = containers.iter()
        .filter_map(|container| container.get("ID").or_else(|| container.get("Id")))
        .filter_map(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let inspected = inspect_containers(&ids);
    attach_inspected(&mut containers, &inspected);
    Ok(containers)
}

#[tauri::command(async)]
fn start_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() || id.contains(' ') || id.contains(';') || id.contains('&') || id.contains('|') {
        return Err("invalid container id".to_string());
    }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "start", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn stop_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "stop", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn restart_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "restart", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn remove_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "rm", "-f", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn logs_container(id: String, tail: Option<u32>) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let t = tail.unwrap_or(100).min(500).to_string();
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "logs", "--tail", &t, &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 && combined.trim().is_empty() { return Err("logs failed".to_string()); }
    let mut s = combined;
    if s.len() > 65536 { s = s[s.len()-65536..].to_string(); }
    Ok(s)
}

#[tauri::command(async)]
fn inspect_container(id: String) -> Result<serde_json::Value, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "inspect", &id])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str::<serde_json::Value>(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

fn stats_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| field.as_str()).map(str::trim).filter(|field| !field.is_empty()).map(str::to_string)
}

#[tauri::command(async)]
fn stats_container(id: String) -> Result<ContainerStats, String> {
    if id.trim().is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("invalid container id".to_string());
    }
    let args = ["--context", "harpoon", "stats", "--no-stream", "--format", "{{json .}}", &id];
    // `docker stats --no-stream` waits for two samples, which alone can take
    // about two seconds over the VM socket.
    let (stdout, stderr, code) = run_docker_bounded(&args, Duration::from_secs(4))?;
    let value = docker_json_lines(&stdout).into_iter().next();
    if code != 0 || value.is_none() {
        return Err(if stderr.trim().is_empty() { "Docker stats returned no sample".to_string() } else { stderr.trim().to_string() });
    }
    let value = value.expect("checked above");
    Ok(ContainerStats {
        source: "docker.stats --no-stream",
        container_id: id,
        cpu_percent: stats_field(&value, "CPUPerc"),
        memory_usage: stats_field(&value, "MemUsage"),
        memory_percent: stats_field(&value, "MemPerc"),
        network_io: stats_field(&value, "NetIO"),
        block_io: stats_field(&value, "BlockIO"),
        pids: stats_field(&value, "PIDs"),
    })
}

#[tauri::command(async)]
fn list_images() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "images", "--format", "{{json .}}", "--no-trunc"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

#[tauri::command(async)]
fn inspect_image(id: String) -> Result<serde_json::Value, String> {
    if id.trim().is_empty() { return Err("invalid image id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "image", "inspect", &id])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command(async)]
fn remove_image(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid image id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "rmi", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

fn list_volumes_basic() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "ls", "--format", "{{json .}}"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

fn attach_volume_users(volumes: &mut [serde_json::Value], inspected: &[serde_json::Value]) {
    let mut attachments: HashMap<String, Vec<String>> = HashMap::new();
    for container in inspected {
        let name = container.get("Name").and_then(|v| v.as_str()).unwrap_or("").trim_start_matches('/').to_string();
        if name.is_empty() { continue; }
        if let Some(mounts) = container.get("Mounts").and_then(|v| v.as_array()) {
            for mount in mounts {
                if mount.get("Type").and_then(|v| v.as_str()) != Some("volume") { continue; }
                if let Some(volume) = mount.get("Name").and_then(|v| v.as_str()) {
                    attachments.entry(volume.to_string()).or_default().push(name.clone());
                }
            }
        }
    }
    for volume in volumes {
        let name = volume.get("Name").and_then(|v| v.as_str()).unwrap_or("");
        let users = attachments.remove(name).unwrap_or_default();
        volume["UsedBy"] = serde_json::Value::Array(users.into_iter().map(serde_json::Value::String).collect());
    }
}

#[tauri::command(async)]
fn list_volumes() -> Result<Vec<serde_json::Value>, String> {
    let mut volumes = list_volumes_basic()?;
    let (ids, _, ids_code) = run_docker(&["--context", "harpoon", "ps", "-aq", "--no-trunc"])?;
    if ids_code == 0 {
        let ids: Vec<&str> = ids.lines().map(str::trim).filter(|id| !id.is_empty()).collect();
        let inspected = inspect_containers(&ids);
        attach_volume_users(&mut volumes, &inspected);
    }
    Ok(volumes)
}

#[tauri::command(async)]
fn inspect_volume(name: String) -> Result<serde_json::Value, String> {
    if name.trim().is_empty() { return Err("invalid volume name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "inspect", &name])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command(async)]
fn remove_volume(name: String) -> Result<String, String> {
    if name.trim().is_empty() { return Err("invalid volume name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "rm", &name])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command(async)]
fn list_networks() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "network", "ls", "--format", "{{json .}}"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

#[tauri::command(async)]
fn inspect_network(name: String) -> Result<serde_json::Value, String> {
    if name.trim().is_empty() { return Err("invalid network name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "network", "inspect", &name])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command(async)]
fn remove_network(name: String) -> Result<String, String> {
    if name.trim().is_empty() { return Err("invalid network name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "network", "rm", &name])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

fn counts_cache() -> &'static Mutex<Option<(serde_json::Value, Instant)>> {
    static CACHE: OnceLock<Mutex<Option<(serde_json::Value, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn count_docker_json_response(stdout: &str, stderr: &str, code: i32, resource: &str) -> Result<usize, String> {
    if code != 0 && !stdout.contains("{") {
        return Err(if stderr.trim().is_empty() {
            format!("Docker {} failed (exit {})", resource, code)
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(docker_json_lines(stdout).len())
}

#[tauri::command(async)]
fn get_counts() -> Result<serde_json::Value, String> {
    // ponytail: coalesce duplicate counts requests (frontend polls status 3.5s + resource tabs); 1.5s TTL avoids duplicate docker storms without daemon polling
    if let Ok(guard) = counts_cache().lock() {
        if let Some((val, at)) = guard.as_ref() {
            if at.elapsed() < Duration::from_millis(1500) {
                return Ok(val.clone());
            }
        }
    }
    // Counting needs no per-container inspection.
    let containers = list_containers_basic(true)?.len();
    let images = list_images()?.len();
    let volumes = list_volumes_basic()?.len();
    let networks = list_networks()?.len();
    let (running_stdout, running_stderr, running_code) = run_docker(&["--context", "harpoon", "ps", "--format", "{{json .}}"])?;
    let running2 = count_docker_json_response(&running_stdout, &running_stderr, running_code, "running containers")?;
    let val = serde_json::json!({
        "containers": containers,
        "running": running2,
        "images": images,
        "volumes": volumes,
        "networks": networks
    });
    if let Ok(mut guard) = counts_cache().lock() {
        *guard = Some((val.clone(), Instant::now()));
    }
    Ok(val)
}

pub(crate) fn refresh_status() -> Result<HarpoonStatus, String> { get_status() }

pub(crate) fn refresh_docker_info() -> Result<String, String> { get_docker_info() }

pub(crate) fn refresh_counts() -> Result<serde_json::Value, String> { get_counts() }

pub(crate) fn refresh_config() -> Result<ConfigResult, String> { get_config() }

#[tauri::command(async)]
fn start_runtime_refresh_worker(app: tauri::AppHandle, config: runtime_refresh::Config) -> Result<u64, String> {
    runtime_refresh::start(app, config)
}

#[tauri::command(async)]
fn stop_runtime_refresh_worker() {
    runtime_refresh::stop();
}

#[tauri::command(async)]
fn start_resource_sampler(app: tauri::AppHandle, disk_path: Option<String>) -> Result<(), String> {
    resource_sampler::start(app, disk_path)
}

#[tauri::command(async)]
fn stop_resource_sampler() {
    resource_sampler::stop();
}

#[tauri::command(async)]
fn start_bootstrap_worker(app: tauri::AppHandle) -> Result<(), String> {
    bootstrap::start(app)
}

#[tauri::command(async)]
fn stop_bootstrap_worker() {
    bootstrap::stop();
}

// --- Stage 3B: Docker host integration ---
#[tauri::command(async)]
fn get_docker_cli_path() -> Result<String, String> {
    let p = resolve_docker_binary()?;
    Ok(p.display().to_string())
}

#[tauri::command(async)]
fn get_docker_compose_info() -> Result<serde_json::Value, String> {
    let bin = resolve_docker_binary().map_err(|e| format!("Docker CLI: FAIL — {}", e))?;
    let out = Command::new(&bin).args(["compose", "version"]).output().map_err(|e| format!("Docker Compose plugin ... FAIL — spawn failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);
    if out.status.success() && (combined.contains("v2") || combined.to_lowercase().contains("compose")) {
        return Ok(serde_json::json!({"installed": true, "version": combined.trim(), "raw": combined.trim()}));
    }
    // Try alternative `docker-compose` binary
    Ok(serde_json::json!({"installed": false, "version": "", "raw": combined.trim(), "hint": "Install Docker Compose v2 plugin (docker compose version). Docker CLI alone is not sufficient for compose workflows."}))
}

fn docker_config_path() -> PathBuf {
    if let Ok(cfg) = std::env::var("DOCKER_CONFIG") {
        if !cfg.trim().is_empty() { return PathBuf::from(cfg).join("config.json"); }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".docker/config.json");
    }
    PathBuf::from("/tmp/.docker/config.json")
}

fn find_helper_in_path(name: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let p = PathBuf::from(dir).join(name);
            if is_executable(&p) { return true; }
        }
    }
    for p in [PathBuf::from("/opt/homebrew/bin").join(name), PathBuf::from("/usr/local/bin").join(name), PathBuf::from("/usr/bin").join(name)] {
        if is_executable(&p) { return true; }
    }
    false
}

#[tauri::command(async)]
fn get_creds_helper_status() -> Result<serde_json::Value, String> {
    let cfg_path = docker_config_path();
    if !cfg_path.exists() {
        return Ok(serde_json::json!({"path": cfg_path.display().to_string(), "credsStore": null, "status": "PASS", "warning": null}));
    }
    let data = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::json!({}));
    let creds = v.get("credsStore").and_then(|x| x.as_str()).unwrap_or("");
    if creds == "desktop" {
        let helper = "docker-credential-desktop";
        let found = find_helper_in_path(helper);
        if !found {
            return Ok(serde_json::json!({"path": cfg_path.display().to_string(), "credsStore": creds, "status": "WARN", "warning": "config references docker-credential-desktop but the helper is not installed. Docker Desktop credsStore will fail — remove or change credsStore in ~/.docker/config.json, or install the helper. Harpoon does not require Docker Desktop."}));
        }
    }
    Ok(serde_json::json!({"path": cfg_path.display().to_string(), "credsStore": if creds.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(creds.to_string()) }, "status": "PASS", "warning": null}))
}

#[tauri::command(async)]
fn ensure_harpoon_context() -> Result<String, String> {
    // Reuse Swift logic via harpoon CLI for single source of truth, but also fallback to direct docker logic if harpoon binary not available.
    // Prefer harpoon binary if resolvable
    if let Ok(bin) = resolve_harpoon_binary() {
        let out = Command::new(&bin).args(["docker", "setup"]).output().map_err(|e| e.to_string())?;
        let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() { return Ok(combined.trim().to_string()); }
        // fall through to direct docker logic for more granular control
        if !combined.contains("not installed") { /* try direct */ }
    }
    let bin = resolve_docker_binary().map_err(|e| format!("Docker CLI: FAIL — {}", e))?;
    let expected = "unix:///tmp/harpoon-docker.sock";
    // inspect
    let inspect_out = Command::new(&bin).args(["context", "inspect", "harpoon"]).output().map_err(|e| e.to_string())?;
    let inspect_stdout = String::from_utf8_lossy(&inspect_out.stdout).to_string();
    if inspect_out.status.success() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&inspect_stdout) {
            if let Some(arr) = v.as_array().and_then(|a| a.first()) {
                if let Some(host) = arr.get("Endpoints").and_then(|e| e.get("docker")).and_then(|d| d.get("Host")).and_then(|h| h.as_str()) {
                    if host == expected {
                        return Ok(format!("Harpoon context already exists and is valid\nEndpoint: {}", host));
                    } else {
                        // repair: update
                        let upd = Command::new(&bin).args(["context", "update", "harpoon", "--docker", &format!("host={}", expected)]).output().map_err(|e| e.to_string())?;
                        if upd.status.success() {
                            return Ok(format!("Repaired Harpoon context {} -> {}", host, expected));
                        }
                        // fallback rm + create
                        let _ = Command::new(&bin).args(["context", "rm", "-f", "harpoon"]).output();
                    }
                }
            }
        }
    }
    // create
    let create = Command::new(&bin).args(["context", "create", "harpoon", "--docker", &format!("host={}", expected), "--description", "Harpoon"]).output().map_err(|e| e.to_string())?;
    let combined = format!("{}{}", String::from_utf8_lossy(&create.stdout), String::from_utf8_lossy(&create.stderr));
    if create.status.success() { return Ok(combined.trim().to_string()); }
    // race check
    let inspect2 = Command::new(&bin).args(["context", "inspect", "harpoon"]).output().map_err(|e| e.to_string())?;
    if inspect2.status.success() { return Ok("Harpoon context now exists (race)".to_string()); }
    Err(combined.trim().to_string())
}

// Stage 3C: disk (via harpoon CLI, single source of truth)
#[tauri::command(async)]
fn get_disk_status() -> Result<String, String> {
    let bin = resolve_harpoon_binary().map_err(|e| e.to_string())?;
    let out = Command::new(&bin).args(["disk", "status"]).output().map_err(|e| e.to_string())?;
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(combined.trim().to_string()) } else { Err(combined.trim().to_string()) }
}

fn resize_disk_blocking(size: String) -> Result<String, String> {
    let bin = resolve_harpoon_binary().map_err(|e| e.to_string())?;
    let out = Command::new(&bin).args(["disk", "resize", &size]).output().map_err(|e| e.to_string())?;
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(combined.trim().to_string()) } else { Err(combined.trim().to_string()) }
}

#[tauri::command(async)]
fn open_shell_session(app: tauri::AppHandle, manager: tauri::State<'_, std::sync::Arc<shell::ShellManager>>, context: String, container_id: String, cols: Option<u16>, rows: Option<u16>) -> Result<shell::ShellSession, String> {
    shell::open(app, manager.inner().clone(), context, container_id, cols, rows)
}

#[tauri::command]
fn write_shell_input(manager: tauri::State<'_, std::sync::Arc<shell::ShellManager>>, session_id: String, input: String) -> Result<(), String> {
    shell::write(manager.inner(), session_id, input)
}

#[tauri::command]
fn resize_shell_session(manager: tauri::State<'_, std::sync::Arc<shell::ShellManager>>, session_id: String, cols: u16, rows: u16) -> Result<(), String> {
    shell::resize(manager.inner(), session_id, cols, rows)
}

#[tauri::command]
fn close_shell_session(manager: tauri::State<'_, std::sync::Arc<shell::ShellManager>>, session_id: String) -> Result<(), String> {
    manager.close(&session_id)
}

#[tauri::command]
async fn run_universal_command(command: String, targets: Vec<broadcast::Target>) -> Result<Vec<broadcast::ResultRow>, String> {
    off_main(move || {
        let docker = resolve_docker_binary()?;
        broadcast::execute(&docker, &command, targets)
    }).await
}

// Lifecycle and disk operations can take a minute; they run on the blocking pool
// rather than holding an async worker for that long.
#[tauri::command]
async fn start_harpoon() -> Result<String, String> { off_main(start_harpoon_blocking).await }

#[tauri::command]
async fn stop_harpoon() -> Result<String, String> { off_main(stop_harpoon_blocking).await }

#[tauri::command]
async fn restart_harpoon() -> Result<String, String> { off_main(restart_harpoon_blocking).await }

#[tauri::command]
async fn resize_disk(size: String) -> Result<String, String> { off_main(move || resize_disk_blocking(size)).await }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_fields_preserve_docker_units_and_missing_values() {
        let value = serde_json::json!({"CPUPerc":"0.12%", "MemUsage":"12.3MiB / 1GiB", "PIDs":"4"});
        assert_eq!(stats_field(&value, "CPUPerc"), Some("0.12%".to_string()));
        assert_eq!(stats_field(&value, "MemUsage"), Some("12.3MiB / 1GiB".to_string()));
        assert_eq!(stats_field(&value, "NetIO"), None);
    }

    #[test]
    fn stats_payload_matches_frontend_camel_case_contract() {
        let payload = ContainerStats {
            source: "docker.stats --no-stream",
            container_id: "abc123".to_string(),
            cpu_percent: Some("0.12%".to_string()),
            memory_usage: Some("12.3MiB / 1GiB".to_string()),
            memory_percent: Some("1.20%".to_string()),
            network_io: Some("1.2kB / 3.4kB".to_string()),
            block_io: Some("0B / 0B".to_string()),
            pids: Some("4".to_string()),
        };
        let json = serde_json::to_value(payload).unwrap();
        assert_eq!(json["cpuPercent"], "0.12%");
        assert_eq!(json["memoryUsage"], "12.3MiB / 1GiB");
        assert_eq!(json["memoryPercent"], "1.20%");
        assert_eq!(json["networkIo"], "1.2kB / 3.4kB");
        assert_eq!(json["blockIo"], "0B / 0B");
        assert_eq!(json["pids"], "4");
        assert!(json.get("cpu_percent").is_none());
    }

    #[test]
    fn stats_fixture_parsing_preserves_partial_and_zero_values() {
        let full = docker_json_lines(include_str!("fixtures/stats/full.jsonl")).into_iter().next().unwrap();
        assert_eq!(stats_field(&full, "CPUPerc"), Some("0.00%".to_string()));
        assert_eq!(stats_field(&full, "NetIO"), Some("0B / 0B".to_string()));
        assert_eq!(stats_field(&full, "BlockIO"), Some("0B / 0B".to_string()));

        let partial = docker_json_lines(include_str!("fixtures/stats/partial.jsonl")).into_iter().next().unwrap();
        assert_eq!(stats_field(&partial, "PIDs"), Some("31".to_string()));
        assert_eq!(stats_field(&partial, "MemUsage"), None);
        assert!(docker_json_lines(include_str!("fixtures/stats/empty.jsonl")).is_empty());
    }

    #[test]
    fn bounded_command_reports_slow_output_without_waiting_forever() {
        let mut command = Command::new("sleep");
        command.arg("1");
        let error = run_command_bounded(&mut command, Duration::from_millis(10)).unwrap_err();
        assert!(error.contains("timed out after 10 ms"));
    }

    #[cfg(unix)]
    #[test]
    fn docker_probe_rejects_broken_and_hung_binaries() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("harpoon-docker-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        assert!(is_runnable_docker(&script("good", "echo 'Docker version 28'")));
        assert!(!is_runnable_docker(&script("broken", "exit 1")));
        let started = Instant::now();
        assert!(!is_runnable_docker(&script("hung", "sleep 10")));
        assert!(started.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_inspect_output_still_enriches_surviving_rows() {
        // `docker inspect live gone` prints live's JSON and exits 1 for gone.
        let inspect_stdout = r#"{"Id":"live-full","Name":"/web","Mounts":[{"Type":"volume","Name":"data","Destination":"/data","RW":true}],"NetworkSettings":{"Networks":{"edge":{}}}}"#;
        let inspected = docker_json_lines(inspect_stdout);
        let mut containers = vec![serde_json::json!({"ID":"live"}), serde_json::json!({"ID":"gone"})];
        attach_inspected(&mut containers, &inspected);
        assert_eq!(containers[0]["NetworkNames"][0], "edge");
        assert!(containers[1].get("NetworkNames").is_none());
        let mut volumes = vec![serde_json::json!({"Name":"data"}), serde_json::json!({"Name":"idle"})];
        attach_volume_users(&mut volumes, &inspected);
        assert_eq!(volumes[0]["UsedBy"][0], "web");
        assert_eq!(volumes[1]["UsedBy"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn failed_inventory_response_is_not_an_empty_count() {
        let error = count_docker_json_response("", "permission denied", 1, "containers").unwrap_err();
        assert_eq!(error, "permission denied");
    }

    #[test]
    fn test_parse_valid_project_service_number() {
        let labels = "com.docker.compose.project=collectiv,com.docker.compose.service=api,com.docker.compose.container-number=1,com.docker.compose.config-hash=abc";
        assert_eq!(parse_compose_label(labels, "com.docker.compose.project"), Some("collectiv".to_string()));
        assert_eq!(parse_compose_label(labels, "com.docker.compose.service"), Some("api".to_string()));
        assert_eq!(parse_compose_label(labels, "com.docker.compose.container-number"), Some("1".to_string()));
    }

    #[test]
    fn test_standalone_empty_labels() {
        let labels = "";
        assert_eq!(parse_compose_label(labels, "com.docker.compose.project"), None);
        let mut v = serde_json::json!({"ID":"abc","Labels":""});
        enrich_compose_fields(&mut v);
        assert!(v.get("ComposeProject").is_none());
    }

    #[test]
    fn test_missing_labels_field() {
        let mut v = serde_json::json!({"ID":"abc","Image":"nginx"});
        enrich_compose_fields(&mut v);
        assert!(v.get("ComposeProject").is_none());
    }

    #[test]
    fn test_unrelated_labels_only() {
        let labels = "foo=bar,other=value";
        assert_eq!(parse_compose_label(labels, "com.docker.compose.project"), None);
        let mut v = serde_json::json!({"Labels": labels});
        enrich_compose_fields(&mut v);
        assert!(v.get("ComposeProject").is_none());
    }

    #[test]
    fn test_malformed_compose_label() {
        // missing value, missing '=', empty project
        let labels = "com.docker.compose.project=,com.docker.compose.service=api";
        assert_eq!(parse_compose_label(labels, "com.docker.compose.project"), None);
        let labels2 = "com.docker.compose.project";
        assert_eq!(parse_compose_label(labels2, "com.docker.compose.project"), None);
        let labels3 = "com.docker.compose.project =  , com.docker.compose.service=api";
        assert_eq!(parse_compose_label(labels3, "com.docker.compose.project"), None);
    }

    #[test]
    fn test_multiple_labels_and_dash_underscore() {
        let labels = "com.docker.compose.project=my-project_1,com.docker.compose.service=api-service_2,com.docker.compose.container-number=2";
        assert_eq!(parse_compose_label(labels, "com.docker.compose.project"), Some("my-project_1".to_string()));
        assert_eq!(parse_compose_label(labels, "com.docker.compose.service"), Some("api-service_2".to_string()));
        let mut v = serde_json::json!({"Labels": labels});
        enrich_compose_fields(&mut v);
        assert_eq!(v.get("ComposeProject").and_then(|x| x.as_str()), Some("my-project_1"));
        assert_eq!(v.get("ComposeService").and_then(|x| x.as_str()), Some("api-service_2"));
        assert_eq!(v.get("ComposeNumber").and_then(|x| x.as_str()), Some("2"));
    }

    #[test]
    fn test_enrich_preserves_labels_and_adds_fields() {
        let labels = "com.docker.compose.project=proj,com.docker.compose.service=web";
        let mut v = serde_json::json!({"ID":"abc","Labels": labels, "Image":"nginx"});
        enrich_compose_fields(&mut v);
        assert_eq!(v.get("Labels").and_then(|x| x.as_str()), Some(labels));
        assert_eq!(v.get("ComposeProject").and_then(|x| x.as_str()), Some("proj"));
        assert_eq!(v.get("ComposeService").and_then(|x| x.as_str()), Some("web"));
        assert!(v.get("ComposeNumber").is_none());
    }

    #[test]
    fn test_labels_as_object_fallback() {
        let mut v = serde_json::json!({"Labels": {"com.docker.compose.project":"proj","com.docker.compose.service":"api"}});
        enrich_compose_fields(&mut v);
        assert_eq!(v.get("ComposeProject").and_then(|x| x.as_str()), Some("proj"));
        assert_eq!(v.get("ComposeService").and_then(|x| x.as_str()), Some("api"));
    }

    #[test]
    fn test_container_attachments_keep_named_and_bind_mounts_distinct() {
        let mut listed = serde_json::json!({"ID":"abc", "ComposeProject":"proj"});
        let inspected = serde_json::json!({
            "Id":"abc-full",
            "Mounts":[
                {"Type":"volume","Name":"proj_db","Destination":"/var/lib/db","RW":true},
                {"Type":"bind","Source":"/tmp/project","Destination":"/app","RW":false}
            ],
            "NetworkSettings":{"Networks":{"proj_default":{},"shared-edge":{}}}
        });
        enrich_container_attachments(&mut listed, &inspected);
        assert_eq!(listed["NetworkNames"].as_array().unwrap().len(), 2);
        assert_eq!(listed["NetworkAttachments"][0]["name"], "proj_default");
        assert_eq!(listed["NamedVolumes"][0]["name"], "proj_db");
        assert_eq!(listed["BindMounts"][0]["source"], "/tmp/project");
        assert_eq!(listed["GroupKind"], "compose");
        assert_eq!(listed["GroupProvenance"], "docker.labels");
    }

}

fn main() {
    let shell_manager = std::sync::Arc::new(shell::ShellManager::new());
    let builder = tauri::Builder::default()
        .manage(shell_manager.clone())
        .invoke_handler(tauri::generate_handler![
            get_status, start_harpoon, stop_harpoon, restart_harpoon,
            get_doctor, get_log_path, get_recent_logs, get_config, set_memory, set_cpus, get_docker_info,
            get_harpoon_binary_path, get_docker_cli_path, get_docker_compose_info, get_creds_helper_status, ensure_harpoon_context, get_disk_status, resize_disk,
            list_containers, start_container, stop_container, restart_container, remove_container, logs_container, inspect_container,
            list_images, inspect_image, remove_image, stats_container,
            list_volumes, inspect_volume, remove_volume,
            list_networks, inspect_network, remove_network,
            context_discovery::discover_contexts,
            get_counts,
            start_runtime_refresh_worker, stop_runtime_refresh_worker,
            start_resource_sampler, stop_resource_sampler,
            start_bootstrap_worker, stop_bootstrap_worker,
            open_shell_session, write_shell_input, resize_shell_session, close_shell_session, run_universal_command
        ]);

    builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_app, event| {
            if matches!(event, tauri::RunEvent::Exit { .. }) {
                shell_manager.close_all();
                resource_sampler::stop();
                runtime_refresh::stop();
            }
        });
}
