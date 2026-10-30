//! Docker Engine API calls, and nothing else. There is deliberately **no** prune, build, push,
//! pull, exec, network or volume call here: the backend cannot make a request this module
//! does not have a method for.

use crate::http::{HttpError, Response, UnixHttp};
use crate::json::{n, obj, s, Json};
use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    Transport(HttpError),
    /// Docker answered with an unexpected status.
    Status {
        op: &'static str,
        status: u16,
        message: String,
    },
    /// Docker answered 2xx but the body is not what the API documents.
    Malformed {
        op: &'static str,
        detail: String,
    },
}

impl ApiError {
    pub fn is_timeout(&self) -> bool {
        matches!(self, ApiError::Transport(HttpError::Timeout(_)))
    }
    /// Docker reports a host port that something else holds.
    pub fn is_port_collision(&self) -> bool {
        match self {
            ApiError::Status { message, .. } => {
                let m = message.to_ascii_lowercase();
                m.contains("port is already allocated") || m.contains("address already in use")
            }
            _ => false,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Transport(e) => write!(f, "{e}"),
            ApiError::Status {
                op,
                status,
                message,
            } => write!(f, "Docker {op} failed with HTTP {status}: {message}"),
            ApiError::Malformed { op, detail } => {
                write!(f, "Docker {op} returned an unexpected response: {detail}")
            }
        }
    }
}

impl std::error::Error for ApiError {}

impl From<HttpError> for ApiError {
    fn from(e: HttpError) -> ApiError {
        ApiError::Transport(e)
    }
}

pub type Labels = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub id: String,
    pub labels: Labels,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub id: String,
    /// Without the leading '/'.
    pub name: String,
    pub labels: Labels,
    pub running: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSummary {
    pub id: String,
    pub name: String,
    pub labels: Labels,
    pub state: String,
    pub host_ports: Vec<u16>,
}

/// Resource limits applied to every container Harpoon creates for a test deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub pids: i64,
}

impl Limits {
    /// First-test limits: 256 MiB, 0.5 CPU, 64 PIDs.
    pub const FIRST_TEST: Limits = Limits {
        memory_bytes: 256 * 1024 * 1024,
        nano_cpus: 500_000_000,
        pids: 64,
    };
}

pub struct CreateSpec<'a> {
    pub image: &'a str,
    pub labels: &'a [(String, String)],
    pub env: &'a [String],
    pub container_port: u16,
    pub host_port: u16,
    pub limits: Limits,
}

impl CreateSpec<'_> {
    /// The create request body. No `HostIp` in the port binding (approved decision), no restart
    /// policy, swap disabled (`MemorySwap == Memory`), default network.
    pub fn body(&self) -> Json {
        let port_key = format!("{}/tcp", self.container_port);
        obj(vec![
            ("Image", s(self.image)),
            (
                "Labels",
                Json::Obj(
                    self.labels
                        .iter()
                        .map(|(k, v)| (k.clone(), s(v.clone())))
                        .collect(),
                ),
            ),
            (
                "Env",
                Json::Arr(self.env.iter().map(|e| s(e.clone())).collect()),
            ),
            (
                "ExposedPorts",
                Json::Obj(vec![(port_key.clone(), obj(vec![]))]),
            ),
            (
                "HostConfig",
                obj(vec![
                    ("Memory", n(self.limits.memory_bytes)),
                    ("MemorySwap", n(self.limits.memory_bytes)),
                    ("NanoCpus", n(self.limits.nano_cpus)),
                    ("PidsLimit", n(self.limits.pids)),
                    ("RestartPolicy", obj(vec![("Name", s("no"))])),
                    (
                        "PortBindings",
                        Json::Obj(vec![(
                            port_key,
                            Json::Arr(vec![obj(vec![("HostPort", s(self.host_port.to_string()))])]),
                        )]),
                    ),
                ]),
            ),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Created(String),
    /// 409: the name is held by something else (not touched).
    NameInUse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogText {
    pub text: String,
    /// The response was cut at the byte cap or ended inside a frame.
    pub truncated: bool,
}

pub struct DockerApi {
    http: UnixHttp,
    timeout: Duration,
}

fn enc(p: &str) -> String {
    let mut out = String::new();
    for b in p.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b':'
            | b'/'
            | b'@' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn message_of(r: &Response) -> String {
    Json::parse(&r.text())
        .ok()
        .and_then(|j| {
            j.get("message")
                .and_then(|m| m.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| r.text().chars().take(300).collect())
}

fn labels_of(j: Option<&Json>, op: &'static str) -> Result<Labels, ApiError> {
    match j {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Obj(m)) => m
            .iter()
            .map(|(k, v)| match v {
                Json::Str(v) => Ok((k.clone(), v.clone())),
                _ => Err(ApiError::Malformed {
                    op,
                    detail: format!("label {k} is not a string"),
                }),
            })
            .collect(),
        Some(_) => Err(ApiError::Malformed {
            op,
            detail: "Labels is not an object".into(),
        }),
    }
}

fn req_str<'a>(j: &'a Json, key: &str, op: &'static str) -> Result<&'a str, ApiError> {
    j.get(key)
        .and_then(Json::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::Malformed {
            op,
            detail: format!("missing or empty \"{key}\""),
        })
}

fn parse(r: &Response, op: &'static str) -> Result<Json, ApiError> {
    Json::parse(&r.text()).map_err(|e| ApiError::Malformed {
        op,
        detail: e.to_string(),
    })
}

impl DockerApi {
    pub fn new(socket: impl Into<std::path::PathBuf>, timeout: Duration) -> DockerApi {
        DockerApi {
            http: UnixHttp::new(socket),
            timeout,
        }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Json>,
        timeout: Duration,
    ) -> Result<Response, ApiError> {
        let b = body.map(|j| j.to_json());
        Ok(self.http.request(
            method,
            path,
            b.as_deref().map(str::as_bytes),
            timeout.min(self.timeout),
        )?)
    }

    fn unexpected(op: &'static str, r: &Response) -> ApiError {
        ApiError::Status {
            op,
            status: r.status,
            message: message_of(r),
        }
    }

    /// The step deadline caps every call: `left` is what the step has remaining.
    pub fn ping(&self, left: Duration) -> Result<(), ApiError> {
        let r = self.call("GET", "/_ping", None, left)?;
        if r.status == 200 && r.text().trim() == "OK" {
            Ok(())
        } else if r.status == 200 {
            Err(ApiError::Malformed {
                op: "ping",
                detail: format!(
                    "expected OK, got {:?}",
                    r.text().chars().take(40).collect::<String>()
                ),
            })
        } else {
            Err(Self::unexpected("ping", &r))
        }
    }

    pub fn image_inspect(
        &self,
        reference: &str,
        left: Duration,
    ) -> Result<Option<ImageInfo>, ApiError> {
        let r = self.call(
            "GET",
            &format!("/images/{}/json", enc(reference)),
            None,
            left,
        )?;
        match r.status {
            200 => {
                let j = parse(&r, "image inspect")?;
                Ok(Some(ImageInfo {
                    id: req_str(&j, "Id", "image inspect")?.to_string(),
                    labels: labels_of(
                        j.get("Config").and_then(|c| c.get("Labels")),
                        "image inspect",
                    )?,
                    tags: j
                        .get("RepoTags")
                        .and_then(Json::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                }))
            }
            404 => Ok(None),
            _ => Err(Self::unexpected("image inspect", &r)),
        }
    }

    /// Every image with its tags and labels (a list entry per tag).
    pub fn list_images(&self, left: Duration) -> Result<Vec<(String, String, Labels)>, ApiError> {
        let r = self.call("GET", "/images/json", None, left)?;
        if r.status != 200 {
            return Err(Self::unexpected("image list", &r));
        }
        let j = parse(&r, "image list")?;
        let arr = j.as_array().ok_or_else(|| ApiError::Malformed {
            op: "image list",
            detail: "not an array".into(),
        })?;
        let mut out = Vec::new();
        for i in arr {
            let id = req_str(i, "Id", "image list")?.to_string();
            let labels = labels_of(i.get("Labels"), "image list")?;
            let tags: Vec<String> = i
                .get("RepoTags")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if tags.is_empty() {
                out.push((id.clone(), id, labels));
            } else {
                for t in tags {
                    out.push((id.clone(), t, labels.clone()));
                }
            }
        }
        Ok(out)
    }

    pub fn list_containers(&self, left: Duration) -> Result<Vec<ContainerSummary>, ApiError> {
        let r = self.call("GET", "/containers/json?all=true", None, left)?;
        if r.status != 200 {
            return Err(Self::unexpected("container list", &r));
        }
        let j = parse(&r, "container list")?;
        let arr = j.as_array().ok_or_else(|| ApiError::Malformed {
            op: "container list",
            detail: "not an array".into(),
        })?;
        let mut out = Vec::new();
        for c in arr {
            let name = c
                .get("Names")
                .and_then(Json::as_array)
                .and_then(|a| a.first())
                .and_then(Json::as_str)
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();
            let host_ports = c
                .get("Ports")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("PublicPort").and_then(Json::as_i64))
                        .filter(|p| (1..=65535).contains(p))
                        .map(|p| p as u16)
                        .collect()
                })
                .unwrap_or_default();
            out.push(ContainerSummary {
                id: req_str(c, "Id", "container list")?.to_string(),
                name,
                labels: labels_of(c.get("Labels"), "container list")?,
                state: c
                    .get("State")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                host_ports,
            });
        }
        Ok(out)
    }

    /// Host ports Docker currently publishes.
    pub fn published_ports(&self, left: Duration) -> Result<BTreeSet<u16>, ApiError> {
        Ok(self
            .list_containers(left)?
            .into_iter()
            .flat_map(|c| c.host_ports)
            .collect())
    }

    pub fn container_inspect(
        &self,
        id_or_name: &str,
        left: Duration,
    ) -> Result<Option<ContainerInfo>, ApiError> {
        let r = self.call(
            "GET",
            &format!("/containers/{}/json", enc(id_or_name)),
            None,
            left,
        )?;
        match r.status {
            200 => {
                let j = parse(&r, "container inspect")?;
                let state = j.get("State").ok_or_else(|| ApiError::Malformed {
                    op: "container inspect",
                    detail: "missing State".into(),
                })?;
                let running = state
                    .get("Running")
                    .and_then(Json::as_bool)
                    .ok_or_else(|| ApiError::Malformed {
                        op: "container inspect",
                        detail: "State.Running is not a boolean".into(),
                    })?;
                Ok(Some(ContainerInfo {
                    id: req_str(&j, "Id", "container inspect")?.to_string(),
                    name: req_str(&j, "Name", "container inspect")?
                        .trim_start_matches('/')
                        .to_string(),
                    labels: labels_of(
                        j.get("Config").and_then(|c| c.get("Labels")),
                        "container inspect",
                    )?,
                    running,
                    exit_code: state
                        .get("ExitCode")
                        .and_then(Json::as_i64)
                        .map(|c| c as i32),
                }))
            }
            404 => Ok(None),
            _ => Err(Self::unexpected("container inspect", &r)),
        }
    }

    pub fn create_container(
        &self,
        name: &str,
        spec: &CreateSpec<'_>,
        left: Duration,
    ) -> Result<CreateOutcome, ApiError> {
        let r = self.call(
            "POST",
            &format!("/containers/create?name={}", enc(name)),
            Some(&spec.body()),
            left,
        )?;
        match r.status {
            201 => {
                let j = parse(&r, "container create")?;
                Ok(CreateOutcome::Created(
                    req_str(&j, "Id", "container create")?.to_string(),
                ))
            }
            409 => Ok(CreateOutcome::NameInUse(message_of(&r))),
            _ => Err(Self::unexpected("container create", &r)),
        }
    }

    /// 204 started, 304 already started — both mean "running".
    pub fn start_container(&self, id: &str, left: Duration) -> Result<(), ApiError> {
        let r = self.call(
            "POST",
            &format!("/containers/{}/start", enc(id)),
            None,
            left,
        )?;
        match r.status {
            204 | 304 => Ok(()),
            _ => Err(Self::unexpected("container start", &r)),
        }
    }

    /// Ok(true) = stopped or already stopped, Ok(false) = no such container.
    pub fn stop_container(
        &self,
        id: &str,
        grace_secs: u32,
        left: Duration,
    ) -> Result<bool, ApiError> {
        let r = self.call(
            "POST",
            &format!("/containers/{}/stop?t={grace_secs}", enc(id)),
            None,
            left,
        )?;
        match r.status {
            204 | 304 => Ok(true),
            404 => Ok(false),
            _ => Err(Self::unexpected("container stop", &r)),
        }
    }

    /// Never `force`, never removes volumes. Ok(true) = removed, Ok(false) = already gone.
    pub fn remove_container(&self, id: &str, left: Duration) -> Result<bool, ApiError> {
        let r = self.call(
            "DELETE",
            &format!("/containers/{}?force=false&v=false", enc(id)),
            None,
            left,
        )?;
        match r.status {
            204 => Ok(true),
            404 => Ok(false),
            _ => Err(Self::unexpected("container remove", &r)),
        }
    }

    /// `noprune=true` so shared parent layers are never deleted as a side effect.
    pub fn remove_image(&self, id: &str, left: Duration) -> Result<bool, ApiError> {
        let r = self.call(
            "DELETE",
            &format!("/images/{}?force=false&noprune=true", enc(id)),
            None,
            left,
        )?;
        match r.status {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(Self::unexpected("image remove", &r)),
        }
    }

    /// At most `tail` lines and `max_bytes` bytes, demultiplexed to text.
    pub fn logs(
        &self,
        id: &str,
        tail: u32,
        max_bytes: usize,
        left: Duration,
    ) -> Result<LogText, ApiError> {
        let path = format!(
            "/containers/{}/logs?stdout=true&stderr=true&tail={tail}",
            enc(id)
        );
        match self.call_capped("GET", &path, max_bytes, left)? {
            Capped::Ok(r) if r.status == 200 => Ok(demux(&r.body)),
            Capped::Ok(r) if r.status == 404 => Ok(LogText {
                text: String::new(),
                truncated: false,
            }),
            Capped::Ok(r) => Err(Self::unexpected("container logs", &r)),
            Capped::TooLarge => Ok(LogText {
                text: String::new(),
                truncated: true,
            }),
        }
    }

    fn call_capped(
        &self,
        method: &str,
        path: &str,
        max: usize,
        left: Duration,
    ) -> Result<Capped, ApiError> {
        let http = self.http.clone().with_max_body(max);
        match http.request(method, path, None, left.min(self.timeout)) {
            Ok(r) => Ok(Capped::Ok(r)),
            Err(HttpError::TooLarge(_)) => Ok(Capped::TooLarge),
            Err(e) => Err(e.into()),
        }
    }
}

enum Capped {
    Ok(Response),
    TooLarge,
}

/// Docker's multiplexed log stream: 8-byte header (stream, 0,0,0, big-endian length) + payload.
/// A body that does not start with a valid header is a TTY stream and is returned as-is.
pub fn demux(body: &[u8]) -> LogText {
    let looks_framed = body.len() >= 8 && body[0] <= 2 && body[1..4] == [0, 0, 0];
    if !looks_framed {
        return LogText {
            text: String::from_utf8_lossy(body).into_owned(),
            truncated: false,
        };
    }
    let mut out = Vec::new();
    let mut i = 0;
    let mut truncated = false;
    while i < body.len() {
        if body.len() - i < 8 || body[i] > 2 || body[i + 1..i + 4] != [0, 0, 0] {
            truncated = true;
            break;
        }
        let len = u32::from_be_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]]) as usize;
        i += 8;
        let end = i.saturating_add(len).min(body.len());
        out.extend_from_slice(&body[i..end]);
        if end - i < len {
            truncated = true;
        }
        i = end;
    }
    LogText {
        text: String::from_utf8_lossy(&out).into_owned(),
        truncated,
    }
}
