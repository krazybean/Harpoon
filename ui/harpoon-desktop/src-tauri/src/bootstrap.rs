use serde_json::json;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const STATUS_EVENT: &str = "bootstrap-status";
const COMPLETE_EVENT: &str = "bootstrap-complete";
const INTERVAL: Duration = Duration::from_millis(750);
const TIMEOUT: Duration = Duration::from_secs(60);

struct Worker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

fn state() -> &'static Mutex<Option<Worker>> {
    static STATE: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn emit_status(app: &AppHandle, status: &crate::HarpoonStatus) {
    let _ = app.emit(STATUS_EVENT, json!({
        "status": status,
        "observedAtMs": now_ms(),
    }));
}

fn emit_complete(app: &AppHandle, ok: bool, error: Option<String>) {
    let _ = app.emit(COMPLETE_EVENT, json!({ "ok": ok, "error": error }));
}

pub fn start(app: AppHandle) -> Result<(), String> {
    stop();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = stop.clone();
    let (wake, wake_rx) = mpsc::channel::<()>();
    let thread = thread::Builder::new().name("harpoon-bootstrap".into()).spawn(move || {
        let started = std::time::Instant::now();
        let initial = match crate::refresh_status() {
            Ok(status) => status,
            Err(error) => { emit_complete(&app, false, Some(error)); return; }
        };
        emit_status(&app, &initial);

        if initial.state == "failed" {
            let message = initial.failure.as_ref().map(|failure| failure.message.clone()).unwrap_or_else(|| "Harpoon reported an authoritative runtime failure.".into());
            emit_complete(&app, false, Some(message));
            return;
        }
        if matches!(initial.state.as_str(), "stopped" | "stale") {
            if let Err(error) = crate::start_harpoon() {
                emit_complete(&app, false, Some(error));
                return;
            }
        }

        loop {
            if worker_stop.load(std::sync::atomic::Ordering::Relaxed) { return; }
            if started.elapsed() >= TIMEOUT {
                emit_complete(&app, false, Some("Harpoon startup timed out.".into()));
                return;
            }
            match crate::refresh_status() {
                Ok(status) => {
                    emit_status(&app, &status);
                    if status.state == "running" && status.docker_ready == Some(true) {
                        emit_complete(&app, true, None);
                        return;
                    }
                    if status.state == "failed" {
                        let message = status.failure.as_ref().map(|failure| failure.message.clone()).unwrap_or_else(|| "Harpoon reported an authoritative runtime failure.".into());
                        emit_complete(&app, false, Some(message));
                        return;
                    }
                }
                Err(error) => {
                    emit_complete(&app, false, Some(error));
                    return;
                }
            }
            if wake_rx.recv_timeout(INTERVAL).is_ok() { return; }
        }
    }).map_err(|error| format!("failed to start bootstrap worker: {error}"))?;
    if let Ok(mut guard) = state().lock() {
        *guard = Some(Worker { stop, wake, thread });
    }
    Ok(())
}

pub fn stop() {
    let worker = state().lock().ok().and_then(|mut guard| guard.take());
    if let Some(worker) = worker {
        worker.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = worker.wake.send(());
        let _ = worker.thread.join();
    }
}
