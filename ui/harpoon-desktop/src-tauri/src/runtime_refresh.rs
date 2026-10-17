use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const INTERVAL: Duration = Duration::from_millis(3500);
const EVENT: &str = "runtime-refresh";

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub status: bool,
    pub docker_info: bool,
    pub counts: bool,
    pub config: bool,
}

struct Worker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: mpsc::Sender<()>,
    generation: u64,
    thread: thread::JoinHandle<()>,
}

fn next_generation() -> u64 {
    static GENERATION: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    GENERATION.get_or_init(|| std::sync::atomic::AtomicU64::new(0)).fetch_add(1, std::sync::atomic::Ordering::Relaxed).saturating_add(1)
}

fn worker_state() -> &'static Mutex<Option<Worker>> {
    static STATE: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn lifecycle_state() -> &'static Mutex<()> {
    static STATE: OnceLock<Mutex<()>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn result(key: &str, value: Result<Value, String>, cycle: u64, generation: u64) -> Value {
    match value {
        Ok(value) => json!({"key": key, "ok": true, "value": value, "cycle": cycle, "generation": generation, "publishedAtMs": now_ms()}),
        Err(error) => json!({"key": key, "ok": false, "error": error, "cycle": cycle, "generation": generation, "publishedAtMs": now_ms()}),
    }
}

fn read(config: Config, cycle: u64, generation: u64) -> Vec<Value> {
    let mut values = Vec::with_capacity(4);
    if config.status {
        values.push(result("get_status", crate::refresh_status().and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())), cycle, generation));
    }
    if config.docker_info {
        values.push(result("get_docker_info", crate::refresh_docker_info().map(Value::String), cycle, generation));
    }
    if config.counts {
        values.push(result("get_counts", crate::refresh_counts(), cycle, generation));
    }
    if config.config {
        values.push(result("get_config", crate::refresh_config().and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())), cycle, generation));
    }
    values
}

fn publish(app: &AppHandle, values: Vec<Value>) {
    for value in values {
        let _ = app.emit(EVENT, value);
    }
}

pub fn start(app: AppHandle, config: Config) -> Result<u64, String> {
    let _lifecycle = lifecycle_state().lock().map_err(|_| "runtime refresh lifecycle lock poisoned".to_string())?;
    stop_locked();
    let generation = next_generation();
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = stopped.clone();
    let (wake, wake_rx) = mpsc::channel::<()>();
    let thread = thread::Builder::new().name("harpoon-runtime-refresh".into()).spawn(move || {
        let mut cycle: u64 = 0;
        while !worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
            if wake_rx.recv_timeout(INTERVAL).is_ok() { break; }
            if worker_stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
            cycle = cycle.saturating_add(1);
            let values = read(config, cycle, generation);
            if worker_stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
            if let Ok(state) = worker_state().lock() {
                if state.as_ref().map(|worker| worker.generation) == Some(generation) {
                    publish(&app, values);
                }
            }
        }
    }).map_err(|error| format!("failed to start runtime refresh worker: {error}"))?;
    if let Ok(mut state) = worker_state().lock() {
        *state = Some(Worker { stop: stopped, wake, generation, thread });
    }
    Ok(generation)
}

pub fn stop() {
    let Ok(_lifecycle) = lifecycle_state().lock() else { return; };
    stop_locked();
}

fn stop_locked() {
    let worker = worker_state().lock().ok().and_then(|mut state| state.take());
    if let Some(worker) = worker {
        worker.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = worker.wake.send(());
        let _ = worker.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publications_are_separate_and_generation_tagged() {
        let value = result("get_status", Ok(json!({"state": "running"})), 3, 9);
        assert_eq!(value["key"], "get_status");
        assert_eq!(value["cycle"], 3);
        assert_eq!(value["ok"], true);
        assert_eq!(value["generation"], 9);
    }

    #[test]
    fn config_keeps_individual_gates() {
        let config: Config = serde_json::from_value(json!({
            "status": true, "dockerInfo": false,
            "counts": true, "config": false
        })).expect("worker config should deserialize");
        assert!(config.status && config.counts);
        assert!(!config.docker_info && !config.config);
    }
}
