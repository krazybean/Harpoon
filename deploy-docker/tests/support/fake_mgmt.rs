//! A fake guest management channel on a test-owned Unix socket: one line of JSON in
//! (`{"op":"exec","argv":[…]}`), one line out, then close — the shape of Harpoon's `harpoon-mgmt`.
//! It records every argv it is asked to run. It runs nothing: answers come from a model.
#![allow(dead_code)]

use harpoon_deploy_docker::json::Json;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub enum Fault {
    /// Reply with exactly these bytes.
    Raw(Vec<u8>),
    /// Accept, read the request, say nothing, hold the connection open.
    Hang,
    /// Close without replying.
    Disconnect,
    /// A well-formed reply with this exit status.
    Exit(i64, String),
}

#[derive(Clone)]
pub struct Model {
    pub hostname: String,
    pub kernel: String,
    pub total: u64,
    pub used: u64,
    pub avail: u64,
    pub mount: String,
    /// When set, `df` prints exactly this instead of a line built from the numbers above.
    pub df_stdout: Option<String>,
    /// argv (space-joined) → fault.
    pub faults: Vec<(String, Fault)>,
}

impl Default for Model {
    fn default() -> Model {
        Model {
            hostname: "harpoon-guest".into(),
            kernel: "6.12.0-test".into(),
            total: 40_000_000_000,
            used: 10_000_000_000,
            avail: 30_000_000_000,
            mount: "/".into(),
            df_stdout: None,
            faults: Vec::new(),
        }
    }
}

pub struct FakeMgmt {
    pub path: PathBuf,
    pub model: Arc<Mutex<Model>>,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

static SEQ: AtomicUsize = AtomicUsize::new(0);

impl FakeMgmt {
    pub fn start() -> FakeMgmt {
        let path = std::env::temp_dir().join(format!(
            "hdm-{}-{}.sock",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        assert!(
            path.starts_with(std::env::temp_dir()),
            "tests only listen on sockets they own"
        );
        let l = UnixListener::bind(&path).expect("bind test mgmt socket");
        l.set_nonblocking(true).unwrap();
        let model = Arc::new(Mutex::new(Model::default()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (m, r, s) = (model.clone(), requests.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((c, _)) => {
                        let _ = c.set_nonblocking(false);
                        let (m, r) = (m.clone(), r.clone());
                        std::thread::spawn(move || handle(c, m, r));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        FakeMgmt {
            path,
            model,
            requests,
            stop,
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn set(&self, f: impl FnOnce(&mut Model)) {
        f(&mut self.model.lock().unwrap());
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeMgmt {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn reply(c: &mut std::os::unix::net::UnixStream, exit: i64, stdout: &str, stderr: &str) {
    let j = Json::Obj(vec![
        ("exit".into(), Json::Num(exit as f64)),
        ("stdout".into(), Json::Str(stdout.into())),
        ("stderr".into(), Json::Str(stderr.into())),
    ]);
    let _ = c.write_all(format!("{}\n", j.to_json()).as_bytes());
}

fn handle(
    mut c: std::os::unix::net::UnixStream,
    model: Arc<Mutex<Model>>,
    requests: Arc<Mutex<Vec<String>>>,
) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
    while !buf.contains(&b'\n') {
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let line = String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let argv: Vec<String> = Json::parse(&line)
        .ok()
        .and_then(|j| {
            j.get("argv").and_then(Json::as_array).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    let key = argv.join(" ");
    requests.lock().unwrap().push(key.clone());
    let m = model.lock().unwrap().clone();
    if let Some((_, f)) = m.faults.iter().find(|(k, _)| *k == key) {
        match f {
            Fault::Raw(b) => {
                let _ = c.write_all(b);
            }
            Fault::Hang => std::thread::sleep(Duration::from_secs(5)),
            Fault::Disconnect => {}
            Fault::Exit(code, err) => reply(&mut c, *code, "", err),
        }
        return;
    }
    match argv
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["hostname"] => reply(&mut c, 0, &format!("{}\n", m.hostname), ""),
        ["uname", "-r"] => reply(&mut c, 0, &format!("{}\n", m.kernel), ""),
        ["df", "-B1", _path] => {
            let out = m.df_stdout.clone().unwrap_or_else(|| {
                let pct = (m.used * 100).checked_div(m.total).unwrap_or(0);
                format!(
                    "Filesystem           1B-blocks         Used    Available Use% Mounted on\n/dev/vda1     {} {} {} {pct}% {}\n",
                    m.total, m.used, m.avail, m.mount
                )
            });
            reply(&mut c, 0, &out, "")
        }
        _ => reply(&mut c, 127, "", "not found"),
    }
}
