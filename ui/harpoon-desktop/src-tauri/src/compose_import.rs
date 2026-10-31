//! Read-only Compose project inspection and explicit, Harpoon-scoped launch.
//! No YAML parser, Docker context switching, or deployment control plane.
use serde::Serialize;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{docker_command, off_main, run_command_bounded};

const CONFIG_TIMEOUT: Duration = Duration::from_secs(15);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(600);
const CONFIG_LIMIT: usize = 2 * 1024 * 1024;
const BASE_FILES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];
const OVERRIDE_FILES: [&str; 4] = ["compose.override.yaml", "compose.override.yml", "docker-compose.override.yaml", "docker-compose.override.yml"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeFolder {
    folder: String,
    files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeService {
    name: String,
    image: Option<String>,
    build: bool,
    ports: Vec<String>,
    mounts: Vec<String>,
    profiles: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposePreview {
    folder: String,
    files: Vec<String>,
    project: String,
    services: Vec<ComposeService>,
    networks: Vec<String>,
    volumes: Vec<String>,
    warnings: Vec<String>,
    existing_containers: usize,
    fingerprint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeLaunchResult {
    project: String,
    detail: String,
}

fn folder_files(root: &Path) -> Result<Vec<String>, String> {
    if !root.is_dir() {
        return Err("Selected location is not a directory".into());
    }
    let mut found = Vec::new();
    for file in BASE_FILES.into_iter().chain(OVERRIDE_FILES) {
        if root.join(file).is_file() { found.push(file.to_owned()); }
    }
    if !found.iter().any(|file| BASE_FILES.contains(&file.as_str())) {
        return Err("No Compose file found in this folder (compose.yaml, compose.yml, docker-compose.yaml or docker-compose.yml)".into());
    }
    Ok(found)
}

fn folder_info(folder: PathBuf) -> Result<ComposeFolder, String> {
    let root = folder.canonicalize().map_err(|_| "Cannot open selected folder".to_owned())?;
    let files = folder_files(&root)?;
    Ok(ComposeFolder { folder: root.to_string_lossy().into_owned(), files })
}

#[tauri::command]
pub async fn pick_compose_folder() -> Result<Option<ComposeFolder>, String> {
    off_main(|| {
        // macOS's standard folder-picker UI, with a fixed AppleScript (no user input
        // interpolated into an executable command).
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", "POSIX path of (choose folder with prompt \"Select a Docker Compose project\")"])
            .output().map_err(|_| "Could not open the macOS folder picker".to_owned())?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            if error.contains("-128") || error.contains("User canceled") {
                return Ok(None);
            }
            return Err("The folder picker did not complete".into());
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        folder_info(PathBuf::from(path)).map(Some)
    }).await
}

fn project_slug(root: &Path) -> Result<String, String> {
    let source = root.file_name().and_then(|name| name.to_str())
        .ok_or("Folder name cannot be used as a Compose project name")?;
    let name: String = source.to_ascii_lowercase().chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_owned();
    if name.is_empty() { return Err("Folder has no valid Compose project name".into()); }
    Ok(name)
}

fn validated_files(folder: &str, files: &[String]) -> Result<(PathBuf, Vec<PathBuf>, String), String> {
    let root = fs::canonicalize(folder).map_err(|_| "Selected folder is no longer accessible".to_owned())?;
    let available = folder_files(&root)?;
    if files.is_empty() || files.len() > 5 { return Err("Choose one base Compose file and optional overrides".into()); }
    let mut selected = Vec::new();
    for (i, name) in files.iter().enumerate() {
        let valid = if i == 0 { BASE_FILES.contains(&name.as_str()) } else { OVERRIDE_FILES.contains(&name.as_str()) };
        if !valid || !available.contains(name) || selected.iter().any(|x: &PathBuf| x.file_name().and_then(|s| s.to_str()) == Some(name.as_str())) {
            return Err("Invalid, duplicate, or missing Compose file".into());
        }
        let resolved = fs::canonicalize(root.join(name)).map_err(|_| "Compose file cannot be read".to_owned())?;
        if resolved.parent() != Some(root.as_path()) || !resolved.is_file() {
            return Err("Compose files must be regular files inside the selected folder".into());
        }
        selected.push(resolved);
    }
    let project = project_slug(&root)?;
    Ok((root, selected, project))
}

fn docker_context_ok() -> Result<(), String> {
    let mut command = docker_command(&["context", "inspect", "harpoon", "--format", "{{.Endpoints.docker.Host}}"])?;
    let (stdout, _, code) = run_command_bounded(&mut command, CONFIG_TIMEOUT)?;
    if code != 0 || stdout.trim() != "unix:///tmp/harpoon-docker.sock" {
        return Err("Harpoon Docker context is unavailable or does not target the expected socket; no changes made".into());
    }
    Ok(())
}

fn compose_command(root: &Path, files: &[PathBuf], project: &str, command: &[&str]) -> Result<Command, String> {
    let mut cmd = docker_command(&["--context", "harpoon", "compose"])?;
    cmd.arg("--project-directory").arg(root).arg("--project-name").arg(project);
    for file in files { cmd.arg("-f").arg(file); }
    cmd.args(command).current_dir(root);
    Ok(cmd)
}

fn config_value(root: &Path, files: &[PathBuf], project: &str) -> Result<(Value, String), String> {
    let mut cmd = compose_command(root, files, project, &["config", "--format", "json"])?;
    let (output, _, code) = run_command_bounded(&mut cmd, CONFIG_TIMEOUT)?;
    if code != 0 { return Err("Compose configuration invalid. Check required variables, referenced files and Compose syntax; no changes made".into()); }
    if output.len() > CONFIG_LIMIT { return Err("Compose configuration exceeds the 2 MB preview limit".into()); }
    let config: Value = serde_json::from_str(&output).map_err(|_| "Compose did not return valid JSON configuration".to_owned())?;
    let mut hasher = DefaultHasher::new();
    output.hash(&mut hasher);
    Ok((config, format!("{:016x}", hasher.finish())))
}

fn string_field(v: &Value, field: &str) -> Option<String> {
    v.get(field).and_then(|x| x.as_str()).map(str::to_owned)
}

fn preview_services(config: &Value) -> Result<(Vec<ComposeService>, Vec<String>), String> {
    let services = config.get("services").and_then(Value::as_object)
        .ok_or("Compose returned no services")?;
    if services.is_empty() { return Err("Compose has no services".into()); }
    let mut warnings = Vec::new();
    let mut result = Vec::new();
    for (name, value) in services {
        let mut ports = Vec::new();
        if let Some(items) = value.get("ports").and_then(Value::as_array) {
            for port in items {
                let published = port.get("published").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| "dynamic".into());
                let target = port.get("target").map(ToString::to_string).unwrap_or_else(|| "?".into());
                let protocol = port.get("protocol").and_then(Value::as_str).unwrap_or("tcp");
                if protocol != "tcp" { warnings.push(format!("{name}: {protocol} forwarding is not supported by Harpoon")); }
                if let Some(ip) = port.get("host_ip").and_then(Value::as_str) {
                    if !ip.is_empty() { warnings.push(format!("{name}: explicit host IP {ip} has not been verified with Harpoon's port forwarder")); }
                }
                ports.push(format!("{published} → {target}/{protocol}"));
            }
        }
        let mut mounts = Vec::new();
        if let Some(items) = value.get("volumes").and_then(Value::as_array) {
            for volume in items {
                let source = string_field(volume, "source").unwrap_or_else(|| "(anonymous)".into());
                let target = string_field(volume, "target").unwrap_or_else(|| "?".into());
                let kind = string_field(volume, "type").unwrap_or_else(|| "volume".into());
                mounts.push(format!("{kind}: {source} → {target}"));
            }
        }
        let profiles = value.get("profiles").and_then(Value::as_array)
            .map(|x| x.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        result.push(ComposeService {
            name: name.clone(),
            image: string_field(value, "image"),
            build: value.get("build").is_some(),
            ports,
            mounts,
            profiles,
        });
    }
    Ok((result, warnings))
}

fn config_names(config: &Value, kind: &str) -> Vec<String> {
    config.get(kind).and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

fn existing_project_containers(project: &str) -> Result<usize, String> {
    let filter = format!("label=com.docker.compose.project={project}");
    let mut cmd = docker_command(&["--context", "harpoon", "ps", "-a", "--filter", &filter, "--format", "{{.ID}}"])?;
    let (stdout, _, code) = run_command_bounded(&mut cmd, CONFIG_TIMEOUT)?;
    if code != 0 { return Err("Cannot inspect existing Compose containers; launch blocked".into()); }
    Ok(stdout.lines().filter(|l| !l.trim().is_empty()).count())
}

fn preview_inner(folder: &str, files: &[String]) -> Result<ComposePreview, String> {
    let (root, paths, project) = validated_files(folder, files)?;
    docker_context_ok()?;
    let (config, fingerprint) = config_value(&root, &paths, &project)?;
    let (services, mut warnings) = preview_services(&config)?;
    let existing = existing_project_containers(&project)?;
    if existing > 0 { warnings.push(format!("{existing} existing container(s) use Compose project '{project}'; launch will be blocked to prevent recreation")); }
    if services.iter().any(|s| !s.profiles.is_empty()) {
        warnings.push("Profile-gated services are shown for inspection; this first version launches only default-profile services".into());
    }
    Ok(ComposePreview {
        folder: root.to_string_lossy().into_owned(),
        files: files.to_owned(), project, services,
        networks: config_names(&config, "networks"),
        volumes: config_names(&config, "volumes"), warnings,
        existing_containers: existing, fingerprint,
    })
}

#[tauri::command]
pub async fn preview_compose_project(folder: String, files: Vec<String>) -> Result<ComposePreview, String> {
    off_main(move || preview_inner(&folder, &files)).await
}

#[tauri::command]
pub async fn launch_compose_project(folder: String, files: Vec<String>, expected_fingerprint: String) -> Result<ComposeLaunchResult, String> {
    off_main(move || {
        let preview = preview_inner(&folder, &files)?;
        if preview.fingerprint != expected_fingerprint {
            return Err("Compose configuration changed since preview. Review it again before launching; no changes made".into());
        }
        if preview.existing_containers != 0 {
            return Err("A Compose stack with this project name already exists. Open it in Containers instead; no changes made".into());
        }
        if preview.warnings.iter().any(|w| w.contains("forwarding is not supported") || w.contains("explicit host IP")) {
            return Err("Compose uses unsupported or unverified port bindings; no changes made".into());
        }
        let (root, paths, project) = validated_files(&folder, &files)?;
        docker_context_ok()?;
        // --no-recreate is a final defensive layer; no force, prune, down or -v.
        // Build/pull is an explicit side effect of the user's Create and Start action.
        let mut command = compose_command(&root, &paths, &project, &["up", "-d", "--build", "--no-recreate", "--quiet-pull", "--quiet-build"])?;
        let (_, _, code) = run_command_bounded(&mut command, LAUNCH_TIMEOUT)?;
        if code != 0 {
            return Err("Compose did not complete successfully. Some resources may have been created or started. Inspect this stack in Containers; nothing was automatically removed or retried".into());
        }
        Ok(ComposeLaunchResult { project, detail: "Compose finished; refresh Docker's actual container state to inspect service health".into() })
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slug_is_deterministic() {
        assert_eq!(project_slug(Path::new("/tmp/My Web.API")).unwrap(), "my-web-api");
        assert!(project_slug(Path::new("/tmp/---")).is_err());
    }
    #[test]
    fn extracts_service_without_environment_values() {
        let data = serde_json::json!({"services":{"web":{
            "image":"nginx:alpine","environment":{"PASSWORD":"do-not-display"},
            "ports":[{"published":"8080","target":80,"protocol":"tcp"}],
            "volumes":[{"type":"bind","source":"/tmp/app","target":"/app"}]
        }}});
        let (services,warnings)=preview_services(&data).unwrap();
        let encoded=serde_json::to_string(&services).unwrap();
        assert!(!encoded.contains("do-not-display"));
        assert_eq!(services[0].ports,vec!["8080 → 80/tcp"]);
        assert!(warnings.is_empty());
    }
    #[test]
    fn flags_udp_and_explicit_host_ip() {
        let data=serde_json::json!({"services":{"udp":{"ports":[{"published":"5353","target":53,"protocol":"udp","host_ip":"127.0.0.1"}]}}});
        let (_,warnings)=preview_services(&data).unwrap();
        assert_eq!(warnings.len(),2);
    }
}
