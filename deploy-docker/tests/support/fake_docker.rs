//! A fake Docker Engine on a test-owned Unix socket. Response shapes follow the Docker Engine API
//! documentation (v1.43 examples); they are hand-recorded from the docs, **not** captured from a
//! live engine. The server refuses any path outside the small set the backend is allowed to use
//! and records every request so tests can assert what was (not) asked.
#![allow(dead_code)]

use harpoon_deploy_docker::json::{obj, Json};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);
/// One counter shared by fake-engine requests and the recording journal, so tests can order them.
pub static CLOCK: AtomicUsize = AtomicUsize::new(0);
pub fn tick() -> usize {
    CLOCK.fetch_add(1, Ordering::SeqCst)
}

pub type Labels = Vec<(String, String)>;

#[derive(Clone, Debug)]
pub struct Ctr {
    pub id: String,
    pub name: String,
    pub labels: Labels,
    pub image: String,
    pub running: bool,
    pub exit_code: i64,
    pub host_port: Option<u16>,
    pub create_body: Json,
    pub logs: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Img {
    pub id: String,
    pub tags: Vec<String>,
    pub labels: Labels,
    pub volumes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    pub seq: usize,
    pub method: String,
    pub path: String,
    pub body: String,
}

#[derive(Clone)]
pub enum Action {
    /// Answer with this status and body instead of the normal handler.
    Respond(u16, String),
    /// Send these raw bytes and close.
    Raw(Vec<u8>),
    /// Promise `claimed` body bytes, send `body`, close.
    Truncate { claimed: usize, body: String },
    /// Close without sending anything.
    Disconnect,
    /// Sleep, then answer normally.
    Delay(Duration),
    /// Mutate the world (a race), then answer normally.
    Race(Arc<dyn Fn(&mut St) + Send + Sync>),
    /// Handle the request normally (the change happens) but never answer: the reply is lost.
    ApplyThenDrop,
    /// Answer 200 with this body using chunked transfer encoding.
    Chunked(String),
}

pub struct Fault {
    pub skip: usize,
    pub method: Option<&'static str>,
    pub path_has: String,
    pub remaining: usize,
    pub action: Action,
}

#[derive(Default)]
pub struct St {
    pub containers: Vec<Ctr>,
    pub images: Vec<Img>,
    pub requests: Vec<Req>,
    pub faults: Vec<Fault>,
    /// Host ports another process "holds": starting a container on one fails like Docker does.
    pub taken_ports: BTreeSet<u16>,
    pub unexpected: Vec<Req>,
    seq: usize,
}

impl St {
    pub fn next_id(&mut self) -> String {
        self.seq += 1;
        // spread the leading digits so unique-prefix lookups (like Docker's) are unambiguous
        format!(
            "{:016x}{:048x}",
            (self.seq as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            self.seq
        )
    }
    pub fn add_image(&mut self, tag: &str, labels: Labels) -> String {
        let id = format!("sha256:{}", self.next_id());
        self.images.push(Img {
            id: id.clone(),
            tags: vec![tag.to_string()],
            labels,
            volumes: Vec::new(),
        });
        id
    }
    pub fn add_container(&mut self, name: &str, labels: Labels, running: bool) -> String {
        let id = self.next_id();
        self.containers.push(Ctr {
            id: id.clone(),
            name: name.to_string(),
            labels,
            image: "x".into(),
            running,
            exit_code: 0,
            host_port: None,
            create_body: Json::Null,
            logs: Vec::new(),
        });
        id
    }
}

pub struct FakeDocker {
    pub socket: PathBuf,
    pub st: Arc<Mutex<St>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn temp_socket() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("hdd-{}-{}.sock", std::process::id(), n));
    let _ = std::fs::remove_file(&p);
    p
}

impl FakeDocker {
    pub fn start() -> FakeDocker {
        let socket = temp_socket();
        // Guard: tests only ever listen on a socket they own under the temp directory.
        assert!(socket.starts_with(std::env::temp_dir()));
        let listener = UnixListener::bind(&socket).expect("bind test socket");
        listener.set_nonblocking(true).unwrap();
        let st = Arc::new(Mutex::new(St::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (st2, stop2) = (st.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        // BSD/macOS accepted sockets inherit O_NONBLOCK from the listener.
                        let _ = conn.set_nonblocking(false);
                        let st = st2.clone();
                        std::thread::spawn(move || handle(conn, st));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        FakeDocker {
            socket,
            st,
            stop,
            thread: Some(thread),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut St) -> R) -> R {
        f(&mut self.st.lock().unwrap())
    }
    pub fn fault(
        &self,
        method: Option<&'static str>,
        path_has: &str,
        remaining: usize,
        action: Action,
    ) {
        self.fault_after(method, path_has, 0, remaining, action)
    }
    /// Let the first `skip` matching requests through, then apply the fault `remaining` times.
    pub fn fault_after(
        &self,
        method: Option<&'static str>,
        path_has: &str,
        skip: usize,
        remaining: usize,
        action: Action,
    ) {
        self.with(|s| {
            s.faults.push(Fault {
                skip,
                method,
                path_has: path_has.to_string(),
                remaining,
                action,
            })
        });
    }
    pub fn requests(&self) -> Vec<Req> {
        self.with(|s| s.requests.clone())
    }
    pub fn container(&self, name: &str) -> Option<Ctr> {
        self.with(|s| s.containers.iter().find(|c| c.name == name).cloned())
    }
    pub fn path(&self) -> &Path {
        &self.socket
    }
}

impl Drop for FakeDocker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn read_request(c: &mut UnixStream) -> Option<(String, String, String)> {
    c.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = c.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let first = lines.next()?;
    let mut it = first.split_whitespace();
    let (method, path) = (it.next()?.to_string(), it.next()?.to_string());
    let len = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = c.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, String::from_utf8_lossy(&body).into_owned()))
}

fn reply(c: &mut UnixStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        304 => "Not Modified",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nApi-Version: 1.43\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = c.write_all(head.as_bytes());
    let _ = c.write_all(body.as_bytes());
}

fn handle(mut c: UnixStream, st: Arc<Mutex<St>>) {
    let Some((method, path, body)) = read_request(&mut c) else {
        return;
    };
    // Pick a fault (if any) before touching the world.
    let fault = {
        let mut s = st.lock().unwrap();
        s.requests.push(Req {
            seq: tick(),
            method: method.clone(),
            path: path.clone(),
            body: body.clone(),
        });
        let mut found = None;
        let mut hit = None;
        for idx in 0..s.faults.len() {
            let f = &mut s.faults[idx];
            if f.remaining > 0
                && path.contains(&f.path_has)
                && f.method.map(|m| m == method).unwrap_or(true)
            {
                if f.skip > 0 {
                    f.skip -= 1;
                    continue;
                }
                hit = Some(idx);
                break;
            }
        }
        if let Some(idx) = hit {
            s.faults[idx].remaining -= 1;
            let action = s.faults[idx].action.clone();
            if let Action::Race(r) = &action {
                r(&mut s);
            } else {
                found = Some(action);
            }
        }
        found
    };
    match fault {
        Some(Action::Respond(status, b)) => return reply(&mut c, status, &b),
        Some(Action::Raw(b)) => {
            let _ = c.write_all(&b);
            return;
        }
        Some(Action::Truncate { claimed, body }) => {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {claimed}\r\n\r\n");
            let _ = c.write_all(head.as_bytes());
            let _ = c.write_all(body.as_bytes());
            return;
        }
        Some(Action::Disconnect) => return,
        Some(Action::ApplyThenDrop) => {
            let _ = route(&st, &method, &path, &body);
            return;
        }
        Some(Action::Chunked(b)) => {
            let mut out = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            for part in b.as_bytes().chunks(7) {
                out.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
                out.extend_from_slice(part);
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"0\r\n\r\n");
            let _ = c.write_all(&out);
            return;
        }
        Some(Action::Delay(d)) => std::thread::sleep(d),
        _ => {}
    }
    if method == "GET"
        && path
            .split('?')
            .next()
            .map(|p| p.ends_with("/logs"))
            .unwrap_or(false)
    {
        let key = path
            .trim_start_matches("/containers/")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let found = st
            .lock()
            .unwrap()
            .containers
            .iter()
            .find(|c| c.id == key || c.name == key)
            .map(|c| c.logs.clone());
        match found {
            Some(payload) => {
                let mut framed = vec![1u8, 0, 0, 0];
                framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
                framed.extend_from_slice(&payload);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.multiplexed-stream\r\nContent-Length: {}\r\n\r\n", framed.len());
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(&framed);
            }
            None => reply(&mut c, 404, "{\"message\":\"No such container\"}"),
        }
        return;
    }
    let (status, out) = route(&st, &method, &path, &body);
    reply(&mut c, status, &out);
}

fn labels_json(l: &Labels) -> Json {
    Json::Obj(
        l.iter()
            .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
            .collect(),
    )
}

pub fn container_inspect_json(c: &Ctr) -> String {
    obj(vec![
        ("Id", Json::Str(c.id.clone())),
        ("Name", Json::Str(format!("/{}", c.name))),
        (
            "State",
            obj(vec![
                (
                    "Status",
                    Json::Str(if c.running { "running" } else { "exited" }.into()),
                ),
                ("Running", Json::Bool(c.running)),
                ("ExitCode", Json::Num(c.exit_code as f64)),
            ]),
        ),
        (
            "Config",
            obj(vec![
                ("Image", Json::Str(c.image.clone())),
                ("Labels", labels_json(&c.labels)),
            ]),
        ),
    ])
    .to_json()
}

fn route(st: &Arc<Mutex<St>>, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = st.lock().unwrap();
    let (p, query) = path.split_once('?').unwrap_or((path, ""));
    let q = |k: &str| {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(str::to_string))
    };
    let not_found =
        |what: &str, id: &str| (404, format!("{{\"message\":\"No such {what}: {id}\"}}"));
    let seg: Vec<&str> = p.trim_start_matches('/').split('/').collect();
    match (method, seg.as_slice()) {
        ("GET", ["_ping"]) => (200, "OK".into()),
        ("GET", ["images", "json"]) => {
            let arr: Vec<Json> = s
                .images
                .iter()
                .map(|i| {
                    obj(vec![
                        ("Id", Json::Str(i.id.clone())),
                        (
                            "RepoTags",
                            Json::Arr(i.tags.iter().map(|t| Json::Str(t.clone())).collect()),
                        ),
                        ("Labels", labels_json(&i.labels)),
                        ("Size", Json::Num(1234.0)),
                    ])
                })
                .collect();
            (200, Json::Arr(arr).to_json())
        }
        ("GET", ["images", rest @ .., "json"]) => {
            let r = rest.join("/");
            match s.images.iter().find(|i| i.id == r || i.tags.contains(&r)) {
                Some(i) => (
                    200,
                    obj(vec![
                        ("Id", Json::Str(i.id.clone())),
                        (
                            "RepoTags",
                            Json::Arr(i.tags.iter().map(|t| Json::Str(t.clone())).collect()),
                        ),
                        (
                            "Config",
                            obj(vec![
                                ("Labels", labels_json(&i.labels)),
                                (
                                    "Volumes",
                                    if i.volumes.is_empty() {
                                        Json::Null
                                    } else {
                                        Json::Obj(
                                            i.volumes
                                                .iter()
                                                .map(|v| (v.clone(), obj(vec![])))
                                                .collect(),
                                        )
                                    },
                                ),
                            ]),
                        ),
                    ])
                    .to_json(),
                ),
                None => not_found("image", &r),
            }
        }
        ("DELETE", ["images", rest @ ..]) => {
            let r = rest.join("/");
            match s
                .images
                .iter()
                .position(|i| i.id == r || i.tags.contains(&r))
            {
                Some(pos) => {
                    let i = s.images.remove(pos);
                    (200, format!("[{{\"Deleted\":\"{}\"}}]", i.id))
                }
                None => not_found("image", &r),
            }
        }
        ("GET", ["containers", "json"]) => {
            let arr: Vec<Json> = s
                .containers
                .iter()
                .map(|c| {
                    let ports = match (c.running, c.host_port) {
                        (true, Some(p)) => vec![obj(vec![
                            ("IP", Json::Str("0.0.0.0".into())),
                            ("PrivatePort", Json::Num(8080.0)),
                            ("PublicPort", Json::Num(p as f64)),
                            ("Type", Json::Str("tcp".into())),
                        ])],
                        _ => vec![],
                    };
                    obj(vec![
                        ("Id", Json::Str(c.id.clone())),
                        ("Names", Json::Arr(vec![Json::Str(format!("/{}", c.name))])),
                        ("Image", Json::Str(c.image.clone())),
                        (
                            "State",
                            Json::Str(if c.running { "running" } else { "exited" }.into()),
                        ),
                        ("Labels", labels_json(&c.labels)),
                        ("Ports", Json::Arr(ports)),
                    ])
                })
                .collect();
            (200, Json::Arr(arr).to_json())
        }
        ("POST", ["containers", "create"]) => {
            let name = q("name").unwrap_or_default();
            if let Some(existing) = s.containers.iter().find(|c| c.name == name) {
                return (
                    409,
                    format!(
                        "{{\"message\":\"Conflict. The container name \\\"/{name}\\\" is already in use by container \\\"{}\\\". You have to remove (or rename) that container to be able to reuse that name.\"}}",
                        existing.id
                    ),
                );
            }
            let Ok(j) = Json::parse(body) else {
                return (400, "{\"message\":\"bad body\"}".into());
            };
            let labels: Labels = j
                .get("Labels")
                .and_then(Json::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let host_port = j
                .get("HostConfig")
                .and_then(|h| h.get("PortBindings"))
                .and_then(Json::as_object)
                .and_then(|m| m.first())
                .and_then(|(_, v)| v.as_array())
                .and_then(|a| a.first())
                .and_then(|b| b.get("HostPort"))
                .and_then(Json::as_str)
                .and_then(|p| p.parse().ok());
            let id = s.next_id();
            s.containers.push(Ctr {
                id: id.clone(),
                name,
                labels,
                image: j
                    .get("Image")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                running: false,
                exit_code: 0,
                host_port,
                create_body: j,
                logs: Vec::new(),
            });
            (201, format!("{{\"Id\":\"{id}\",\"Warnings\":[]}}"))
        }
        (m, ["containers", id, rest @ ..]) => {
            let pos = s
                .containers
                .iter()
                .position(|c| c.id == *id || c.name == *id)
                .or_else(|| {
                    // like Docker: a unique id prefix also resolves
                    let m: Vec<usize> = s
                        .containers
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| id.len() >= 4 && c.id.starts_with(*id))
                        .map(|(i, _)| i)
                        .collect();
                    (m.len() == 1).then(|| m[0])
                });
            let Some(pos) = pos else {
                return not_found("container", id);
            };
            match (m, rest) {
                ("GET", ["json"]) => (200, container_inspect_json(&s.containers[pos])),
                ("POST", ["start"]) => {
                    if let Some(p) = s.containers[pos].host_port {
                        if s.taken_ports.contains(&p) {
                            return (
                                500,
                                format!("{{\"message\":\"driver failed programming external connectivity on endpoint x: Bind for 0.0.0.0:{p} failed: port is already allocated\"}}"),
                            );
                        }
                    }
                    if s.containers[pos].running {
                        return (304, String::new());
                    }
                    s.containers[pos].running = true;
                    (204, String::new())
                }
                ("POST", ["stop"]) => {
                    if !s.containers[pos].running {
                        return (304, String::new());
                    }
                    s.containers[pos].running = false;
                    (204, String::new())
                }
                ("DELETE", []) => {
                    if s.containers[pos].running {
                        return (
                            409,
                            "{\"message\":\"cannot remove a running container: stop the container before removing or force remove\"}".into(),
                        );
                    }
                    s.containers.remove(pos);
                    (204, String::new())
                }
                _ => {
                    s.unexpected.push(Req {
                        seq: tick(),
                        method: method.into(),
                        path: path.into(),
                        body: body.into(),
                    });
                    (404, "{\"message\":\"page not found\"}".into())
                }
            }
        }
        _ => {
            s.unexpected.push(Req {
                seq: tick(),
                method: method.into(),
                path: path.into(),
                body: body.into(),
            });
            (404, "{\"message\":\"page not found\"}".into())
        }
    }
}
