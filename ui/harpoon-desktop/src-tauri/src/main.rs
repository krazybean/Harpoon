#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    // 2. Bundled application resource seam (preferred for production Harpoon.app)
    if let Some(bundled) = bundled_harpoon_path() {
        return Ok(bundled);
    }
    // 3. Development checkout derived from CARGO_MANIFEST_DIR (not CWD)
    //    src-tauri lives at <repo>/ui/harpoon-desktop/src-tauri
    //    development binary is <repo>/harpoon/build/harpoon
    let dev_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../harpoon/build/harpoon");
    if is_executable(&dev_bin) {
        if let Ok(c) = dev_bin.canonicalize() { return Ok(c); }
        return Ok(dev_bin);
    }
    // Also try canonicalize even if not executable bit yet (maybe built but not chmod)
    if let Ok(c) = dev_bin.canonicalize() {
        if c.exists() { return Ok(c); }
    }
    // Also try dev_bin without canonicalize as fallback (for sandbox)
    if dev_bin.exists() { return Ok(dev_bin); }
    // 4. /usr/local/bin/harpoon
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
    // ponytail: explicit, no `zsh -l -c`, no sourcing startup files.
    // Finder-launched apps have minimal PATH (e.g. /usr/bin:/bin:/usr/sbin:/sbin).
    // We safely consider: 1) current PATH, 2) /opt/homebrew/bin/docker, 3) /usr/local/bin/docker, 4) other standard locations.
    // Return absolute path for diagnostics; callers must use this, not bare `docker`.
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
            if is_executable(&p) { return Ok(p); }
        }
    }
    // 2-4) Check standard locations regardless of PATH (Finder fallback)
    for p in &candidates {
        if is_executable(p) { return Ok(p.clone()); }
    }
    // Also try `which` with extended PATH that includes homebrew locations (still no shell sourcing)
    let extended = format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}", std::env::var("PATH").unwrap_or_default());
    for dir in extended.split(':') {
        if dir.is_empty() { continue; }
        let p = PathBuf::from(dir).join("docker");
        if is_executable(&p) { return Ok(p); }
    }
    Err("Docker CLI not installed/found (checked PATH, /opt/homebrew/bin/docker, /usr/local/bin/docker, /usr/bin/docker)".to_string())
}

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

#[tauri::command]
fn get_harpoon_binary_path() -> Result<String, String> {
    let p = resolve_harpoon_binary()?;
    Ok(p.display().to_string())
}

#[tauri::command]
fn get_status() -> Result<HarpoonStatus, String> {
    let (stdout, stderr, code) = run_harpoon(&["status", "--json"])?;
    if code != 0 && stdout.trim().is_empty() {
        return Err(format!("harpoon status --json failed (code {}): {}", code, stderr));
    }
    let json_str = if stdout.trim().is_empty() { stderr } else { stdout };
    serde_json::from_str(&json_str).map_err(|e| format!("malformed status JSON: {} | raw: {}", e, json_str))
}

#[tauri::command]
fn start_harpoon() -> Result<String, String> {
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

#[tauri::command]
fn stop_harpoon() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["stop"])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 && !combined.to_lowercase().contains("already stopped") && !combined.to_lowercase().contains("not running") {
        return Err(combined.trim().to_string());
    }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn restart_harpoon() -> Result<String, String> {
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

#[tauri::command]
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

#[tauri::command]
fn get_log_path() -> Result<String, String> {
    let (stdout, stderr, code) = run_harpoon(&["logs", "--path"])?;
    let combined = format!("{}{}", stdout, stderr);
    let path = combined.lines().next().unwrap_or("").trim().to_string();
    if path.is_empty() {
        return Err(format!("logs --path failed (code {}): {}", code, combined));
    }
    Ok(path)
}

#[tauri::command]
fn get_recent_logs(lines: Option<u32>) -> Result<String, String> {
    let n = lines.unwrap_or(200).min(500);
    let (stdout, _stderr, _code) = run_harpoon(&["logs", "--lines", &n.to_string()])?;
    let mut s = stdout;
    if s.len() > 65536 {
        s = s[s.len() - 65536..].to_string();
    }
    Ok(s)
}

#[tauri::command]
fn get_config() -> Result<ConfigResult, String> {
    let (stdout, stderr, code) = run_harpoon(&["config", "show"])?;
    let raw = format!("{}{}", stdout, stderr);
    if code != 0 && !raw.contains("cpus") {
        return Err(raw);
    }
    let mut cpus = 2;
    let mut memory = 1024;
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

#[tauri::command]
fn set_memory(memory: u32) -> Result<String, String> {
    let allowed = [512, 768, 1024, 1536, 2048];
    if !allowed.contains(&memory) { return Err(format!("unsupported memory {}, allowed {:?}", memory, allowed)); }
    let (stdout, stderr, code) = run_harpoon(&["config", "set", "memory", &memory.to_string()])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined); }
    Ok(combined)
}

#[tauri::command]
fn set_cpus(cpus: u32) -> Result<String, String> {
    let allowed = [1, 2, 4];
    if !allowed.contains(&cpus) { return Err(format!("unsupported cpus {}, allowed {:?}", cpus, allowed)); }
    let (stdout, stderr, code) = run_harpoon(&["config", "set", "cpus", &cpus.to_string()])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined); }
    Ok(combined)
}

#[tauri::command]
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

#[tauri::command]
fn list_containers(all: Option<bool>) -> Result<Vec<serde_json::Value>, String> {
    let mut args = vec!["--context", "harpoon", "ps"];
    if all.unwrap_or(true) { args.push("-a"); }
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

#[tauri::command]
fn start_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() || id.contains(' ') || id.contains(';') || id.contains('&') || id.contains('|') {
        return Err("invalid container id".to_string());
    }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "start", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn stop_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "stop", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn restart_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "restart", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn remove_container(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "rm", "-f", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
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

#[tauri::command]
fn inspect_container(id: String) -> Result<serde_json::Value, String> {
    if id.trim().is_empty() { return Err("invalid container id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "inspect", &id])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str::<serde_json::Value>(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command]
fn list_images() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "images", "--format", "{{json .}}", "--no-trunc"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

#[tauri::command]
fn inspect_image(id: String) -> Result<serde_json::Value, String> {
    if id.trim().is_empty() { return Err("invalid image id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "image", "inspect", &id])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command]
fn remove_image(id: String) -> Result<String, String> {
    if id.trim().is_empty() { return Err("invalid image id".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "rmi", &id])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn list_volumes() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "ls", "--format", "{{json .}}"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

#[tauri::command]
fn inspect_volume(name: String) -> Result<serde_json::Value, String> {
    if name.trim().is_empty() { return Err("invalid volume name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "inspect", &name])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command]
fn remove_volume(name: String) -> Result<String, String> {
    if name.trim().is_empty() { return Err("invalid volume name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "volume", "rm", &name])?;
    let combined = format!("{}{}", stdout, stderr);
    if code != 0 { return Err(combined.trim().to_string()); }
    Ok(combined.trim().to_string())
}

#[tauri::command]
fn list_networks() -> Result<Vec<serde_json::Value>, String> {
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "network", "ls", "--format", "{{json .}}"])?;
    if code != 0 && !stdout.contains("{") { return Err(stderr.trim().to_string()); }
    Ok(docker_json_lines(&stdout))
}

#[tauri::command]
fn inspect_network(name: String) -> Result<serde_json::Value, String> {
    if name.trim().is_empty() { return Err("invalid network name".to_string()); }
    let (stdout, stderr, code) = run_docker(&["--context", "harpoon", "network", "inspect", &name])?;
    if code != 0 { return Err(stderr.trim().to_string()); }
    serde_json::from_str(&stdout).map_err(|e| format!("inspect parse failed: {}", e))
}

#[tauri::command]
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

#[tauri::command]
fn get_counts() -> Result<serde_json::Value, String> {
    // ponytail: coalesce duplicate counts requests (frontend polls status 3.5s + resource tabs); 1.5s TTL avoids duplicate docker storms without daemon polling
    if let Ok(guard) = counts_cache().lock() {
        if let Some((val, at)) = guard.as_ref() {
            if at.elapsed() < Duration::from_millis(1500) {
                return Ok(val.clone());
            }
        }
    }
    let containers = list_containers(Some(true)).unwrap_or_default().len();
    let images = list_images().unwrap_or_default().len();
    let volumes = list_volumes().unwrap_or_default().len();
    let networks = list_networks().unwrap_or_default().len();
    let running2 = {
        let (stdout, _, _) = run_docker(&["--context", "harpoon", "ps", "--format", "{{json .}}"]).unwrap_or(("".to_string(),"".to_string(),0));
        docker_json_lines(&stdout).len()
    };
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

// --- Stage 3B: Docker host integration ---
#[tauri::command]
fn get_docker_cli_path() -> Result<String, String> {
    let p = resolve_docker_binary()?;
    Ok(p.display().to_string())
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
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
#[tauri::command]
fn get_disk_status() -> Result<String, String> {
    let bin = resolve_harpoon_binary().map_err(|e| e.to_string())?;
    let out = Command::new(&bin).args(["disk", "status"]).output().map_err(|e| e.to_string())?;
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(combined.trim().to_string()) } else { Err(combined.trim().to_string()) }
}

#[tauri::command]
fn resize_disk(size: String) -> Result<String, String> {
    let bin = resolve_harpoon_binary().map_err(|e| e.to_string())?;
    let out = Command::new(&bin).args(["disk", "resize", &size]).output().map_err(|e| e.to_string())?;
    let combined = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(combined.trim().to_string()) } else { Err(combined.trim().to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status, start_harpoon, stop_harpoon, restart_harpoon,
            get_doctor, get_log_path, get_recent_logs, get_config, set_memory, set_cpus, get_docker_info,
            get_harpoon_binary_path, get_docker_cli_path, get_docker_compose_info, get_creds_helper_status, ensure_harpoon_context, get_disk_status, resize_disk,
            list_containers, start_container, stop_container, restart_container, remove_container, logs_container, inspect_container,
            list_images, inspect_image, remove_image,
            list_volumes, inspect_volume, remove_volume,
            list_networks, inspect_network, remove_network,
            get_counts
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
