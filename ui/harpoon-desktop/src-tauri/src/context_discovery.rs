use serde::Serialize;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub enum DiscoveryState {
    #[serde(rename = "detected")] Detected,
    #[serde(rename = "connected")] Connected,
    #[serde(rename = "unavailable")] Unavailable,
    #[serde(rename = "authentication_needed")] AuthenticationNeeded,
}

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct DiscoveryCapabilities {
    pub inspect: String,
    pub containers: String,
    pub images: String,
    pub volumes: String,
    pub networks: String,
    pub mutations: String,
}

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct DiscoveredContext {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub endpoint: Option<String>,
    pub source: String,
    pub state: DiscoveryState,
    pub current: bool,
    pub harpoon: bool,
    pub detail: String,
    pub capabilities: DiscoveryCapabilities,
}

fn unknown_capabilities() -> DiscoveryCapabilities {
    DiscoveryCapabilities { inspect: "unknown".into(), containers: "unknown".into(), images: "unknown".into(), volumes: "unknown".into(), networks: "unknown".into(), mutations: "unsupported".into() }
}

fn connected_capabilities() -> DiscoveryCapabilities {
    DiscoveryCapabilities { inspect: "available".into(), containers: "available".into(), images: "available".into(), volumes: "available".into(), networks: "available".into(), mutations: "unsupported".into() }
}

fn auth_error(text: &str) -> bool {
    let text = text.to_lowercase();
    ["authentication", "unauthorized", "credentials", "login required", "forbidden"].iter().any(|needle| text.contains(needle))
}

fn run_bounded(mut command: Command) -> Result<(String, String, i32), String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            let mut stdout = String::new();
            let mut stderr = String::new();
            child.stdout.take().map(|mut out| out.read_to_string(&mut stdout));
            child.stderr.take().map(|mut out| out.read_to_string(&mut stderr));
            return Ok((stdout, stderr, status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out after 2 seconds".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn json_lines(output: &str) -> Vec<serde_json::Value> {
    output.lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).collect()
}

fn endpoint_key(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if let Some(path) = endpoint.strip_prefix("unix://localhost") { return format!("unix://{}", path); }
    endpoint.to_string()
}

fn context(name: String, endpoint: Option<String>, current: bool, state: DiscoveryState, detail: String, harpoon: bool) -> DiscoveredContext {
    let connected = state == DiscoveryState::Connected;
    DiscoveredContext { id: endpoint.as_deref().map(endpoint_key).unwrap_or_else(|| name.clone()), name, engine: "docker".into(), endpoint, source: "docker context".into(), state, current, harpoon, detail, capabilities: if connected { connected_capabilities() } else { unknown_capabilities() } }
}

fn docker_contexts(binary: &Path) -> Vec<DiscoveredContext> {
    let mut command = Command::new(binary);
    command.args(["context", "ls", "--format", "{{json .}}"]).env("DOCKER_CLI_HINTS", "false");
    let Ok((stdout, _stderr, code)) = run_bounded(command) else { return Vec::new() };
    if code != 0 { return Vec::new(); }
    let current = Command::new(binary).args(["context", "show"]).env("DOCKER_CLI_HINTS", "false").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_string());
    json_lines(&stdout).into_iter().filter_map(|row| {
        let name = row.get("Name").and_then(|v| v.as_str())?.trim().to_string();
        let endpoint = row.get("DockerEndpoint").and_then(|v| v.as_str()).map(str::to_string).filter(|s| !s.is_empty());
        let error = row.get("Error").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let is_current = current.as_deref() == Some(name.as_str()) || row.get("Current").and_then(|v| v.as_bool()).unwrap_or(false);
        let state = if auth_error(&error) { DiscoveryState::AuthenticationNeeded } else if error.is_empty() { DiscoveryState::Detected } else { DiscoveryState::Unavailable };
        Some(context(name.clone(), endpoint, is_current, state, if error.is_empty() { "Context configuration discovered; endpoint not contacted yet.".into() } else { error }, name == "harpoon"))
    }).collect()
}

fn probe_docker(binary: &Path, name: String, endpoint: Option<String>, current: bool, source: String, harpoon: bool) -> DiscoveredContext {
    let mut command = Command::new(binary);
    if let Some(endpoint) = endpoint.as_deref() { command.args(["--host", endpoint]); } else { command.args(["--context", &name]); }
    command.args(["version", "--format", "{{json .}}"]);
    match run_bounded(command) {
        Ok((stdout, _stderr, code)) if code == 0 && stdout.contains("Server") => {
            let mut result = context(name, endpoint, current, DiscoveryState::Connected, "Docker Engine responded to a read-only version probe.".into(), harpoon);
            result.source = source;
            result
        }
        Ok((_stdout, stderr, _)) if auth_error(&stderr) => { let mut result = context(name, endpoint, current, DiscoveryState::AuthenticationNeeded, stderr.trim().to_string(), harpoon); result.source = source; result }
        Ok((_stdout, stderr, _)) => { let mut result = context(name, endpoint, current, DiscoveryState::Unavailable, stderr.trim().to_string(), harpoon); result.source = source; result }
        Err(error) => { let mut result = context(name, endpoint, current, DiscoveryState::Unavailable, error, harpoon); result.source = source; result }
    }
}

fn socket_candidates() -> Vec<String> {
    let mut sockets = Vec::new();
    if let Ok(host) = std::env::var("DOCKER_HOST") { if host.starts_with("unix://") { sockets.push(host); } }
    for path in ["/var/run/docker.sock", "/tmp/docker.sock", "/tmp/harpoon-docker.sock", "/var/run/podman/podman.sock"] { sockets.push(format!("unix://{path}")); }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") { sockets.push(format!("unix://{runtime}/podman/podman.sock")); }
    sockets
}

fn find_binary(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var("PATH").ok()?;
    path.split(':').map(|dir| Path::new(dir).join(name)).find(|candidate| candidate.is_file())
}

#[tauri::command]
pub fn discover_contexts() -> Result<Vec<DiscoveredContext>, String> {
    let mut discovered = Vec::new();
    if let Ok(binary) = crate::resolve_docker_binary() {
        for item in docker_contexts(&binary) {
            if let Some(endpoint) = item.endpoint.clone() {
                discovered.push(probe_docker(&binary, item.name, Some(endpoint), item.current, item.source, item.harpoon));
            } else { discovered.push(item); }
        }
        for endpoint in socket_candidates() {
            if !discovered.iter().any(|item| item.endpoint.as_deref().map(endpoint_key).as_deref() == Some(endpoint_key(&endpoint).as_str())) {
                discovered.push(probe_docker(&binary, endpoint.clone(), Some(endpoint.clone()), false, "local Unix socket".into(), endpoint.ends_with("/tmp/harpoon-docker.sock")));
            }
        }
    } else {
        discovered.push(DiscoveredContext { id: "docker-cli".into(), name: "Docker CLI".into(), engine: "docker".into(), endpoint: None, source: "executable probe".into(), state: DiscoveryState::Unavailable, current: false, harpoon: false, detail: "Docker CLI not installed or not runnable from the application environment.".into(), capabilities: unknown_capabilities() });
    }
    if let Some(podman) = find_binary("podman") {
        let mut command = Command::new(&podman);
        command.args(["system", "connection", "list", "--format", "json"]);
        if let Ok((stdout, _, code)) = run_bounded(command) {
            if code == 0 { for row in json_lines(&stdout) {
                let name = row.get("Name").or_else(|| row.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let endpoint = row.get("URI").or_else(|| row.get("uri")).and_then(|v| v.as_str()).map(str::to_string);
                if !name.is_empty() && endpoint.is_some() && !discovered.iter().any(|item| item.endpoint.as_deref().map(endpoint_key).as_deref() == endpoint.as_deref().map(endpoint_key).as_deref()) {
                    discovered.push(DiscoveredContext { id: endpoint.as_deref().map(endpoint_key).unwrap_or_else(|| name.clone()), name, engine: "podman".into(), endpoint, source: "podman system connection".into(), state: DiscoveryState::Detected, current: false, harpoon: false, detail: "Podman connection discovered; no engine operation was attempted.".into(), capabilities: unknown_capabilities() });
                }
            }}
        }
    }
    let mut seen = HashSet::new();
    discovered.retain(|item| seen.insert(item.id.clone()));
    Ok(discovered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_states_preserve_partial_data_and_auth() {
        let missing: Vec<DiscoveredContext> = serde_json::from_str(include_str!("fixtures/context-discovery/missing-executable.json")).unwrap();
        let dead: Vec<DiscoveredContext> = serde_json::from_str(include_str!("fixtures/context-discovery/dead-socket.json")).unwrap();
        let auth: Vec<DiscoveredContext> = serde_json::from_str(include_str!("fixtures/context-discovery/auth-needed.json")).unwrap();
        assert_eq!(missing[0].state, DiscoveryState::Unavailable);
        assert_eq!(dead[0].state, DiscoveryState::Unavailable);
        assert_eq!(auth[0].state, DiscoveryState::AuthenticationNeeded);
        assert_eq!(auth[0].capabilities.mutations, "unsupported");
        assert!(dead[0].endpoint.is_some());
    }

    #[test]
    fn duplicate_endpoint_fixture_has_one_identity() {
        let rows: Vec<DiscoveredContext> = serde_json::from_str(include_str!("fixtures/context-discovery/duplicate-endpoints.json")).unwrap();
        let unique: HashSet<String> = rows.iter().filter_map(|row| row.endpoint.as_deref()).map(endpoint_key).collect();
        assert_eq!(unique.len(), 1);
    }
}
