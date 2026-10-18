use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const PUBLISH_EVERY: usize = 2;
pub const HISTORY_LIMIT: usize = 180;
const EVENT: &str = "resource-sample";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSample {
    pub timestamp_ms: u128,
    pub scope: &'static str,
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub disk_logical_bytes: Option<u64>,
    pub disk_allocated_bytes: Option<u64>,
    pub availability: Availability,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub host_cpu: &'static str,
    pub host_memory: &'static str,
    pub image_allocation: &'static str,
    pub disk_activity: &'static str,
    pub vm_guest: &'static str,
    pub container: &'static str,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct ResourceSnapshot {
    source: &'static str,
    unit: &'static str,
    samples: Vec<ResourceSample>,
}

struct Worker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: thread::JoinHandle<()>,
}

fn worker_state() -> &'static Mutex<Option<Worker>> {
    static STATE: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[derive(Default)]
struct CpuTicks {
    user: u64,
    system: u64,
    idle: u64,
    nice: u64,
}

impl CpuTicks {
    fn utilization_since(&self, previous: &Self) -> Option<f64> {
        let total = self.user.checked_sub(previous.user)?
            .checked_add(self.system.checked_sub(previous.system)?)?
            .checked_add(self.idle.checked_sub(previous.idle)?)?
            .checked_add(self.nice.checked_sub(previous.nice)?)?;
        if total == 0 { return None; }
        let idle = self.idle.checked_sub(previous.idle)? as f64;
        Some(((total as f64 - idle) / total as f64 * 100.0).clamp(0.0, 100.0))
    }
}

#[cfg(target_os = "macos")]
fn host_cpu_ticks() -> Option<CpuTicks> {
    use libc::{c_int, c_uint};
    const HOST_CPU_LOAD_INFO: c_int = 3;
    const KERN_SUCCESS: c_int = 0;
    extern "C" {
        fn mach_host_self() -> c_uint;
        fn host_statistics(host: c_uint, flavor: c_int, info: *mut c_int, count: *mut c_uint) -> c_int;
    }
    let mut ticks = [0u32; 4];
    let mut count = ticks.len() as c_uint;
    let result = unsafe { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, ticks.as_mut_ptr() as *mut c_int, &mut count) };
    if result != KERN_SUCCESS || count < 4 { return None; }
    Some(CpuTicks { user: ticks[0] as u64, system: ticks[1] as u64, idle: ticks[2] as u64, nice: ticks[3] as u64 })
}

#[cfg(not(target_os = "macos"))]
fn host_cpu_ticks() -> Option<CpuTicks> { None }

#[cfg(target_os = "macos")]
fn host_memory() -> Option<(u64, u64)> {
    use libc::{c_int, c_uint};
    const HOST_VM_INFO64: c_int = 4;
    const KERN_SUCCESS: c_int = 0;
    extern "C" {
        fn mach_host_self() -> c_uint;
        fn host_statistics64(host: c_uint, flavor: c_int, info: *mut c_int, count: *mut c_uint) -> c_int;
    }
    // host_statistics64 reports an array of 32-bit natural_t page counters.
    // Reading it as u64 combines adjacent counters and can make free memory
    // absurdly large, collapsing the displayed used value to zero.
    let mut stats = [0u32; 40];
    let mut count = stats.len() as c_uint;
    let result = unsafe { host_statistics64(mach_host_self(), HOST_VM_INFO64, stats.as_mut_ptr() as *mut c_int, &mut count) };
    if result != KERN_SUCCESS || count == 0 { return None; }
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let total = sysctl_u64("hw.memsize")?;
    memory_from_page_stats(&stats[..(count as usize).min(stats.len())], page_size, total)
}

#[cfg(not(target_os = "macos"))]
fn host_memory() -> Option<(u64, u64)> { None }

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    use std::ffi::CString;
    let key = CString::new(name).ok()?;
    let mut value = 0u64;
    let mut size = std::mem::size_of::<u64>();
    let result = unsafe { libc::sysctlbyname(key.as_ptr(), (&mut value as *mut u64).cast(), &mut size, std::ptr::null_mut(), 0) };
    (result == 0 && size == std::mem::size_of::<u64>()).then_some(value)
}

#[cfg(not(target_os = "macos"))]
fn sysctl_u64(_name: &str) -> Option<u64> { None }

fn memory_from_page_stats(stats: &[u32], page_size: i64, total: u64) -> Option<(u64, u64)> {
    if stats.is_empty() || page_size <= 0 || total == 0 { return None; }
    let free = u64::from(stats[0]).checked_mul(page_size as u64)?;
    Some((total.saturating_sub(free), total))
}

fn image_allocation(path: Option<&PathBuf>) -> (Option<u64>, Option<u64>, &'static str) {
    let Some(path) = path else { return (None, None, "unavailable"); };
    let Ok(metadata) = std::fs::metadata(path) else { return (None, None, "unavailable"); };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return (Some(metadata.len()), Some(metadata.blocks().saturating_mul(512)), "available");
    }
    #[cfg(not(unix))]
    { (Some(metadata.len()), None, "partial") }
}

fn sample(previous_cpu: Option<&CpuTicks>, disk_path: Option<&PathBuf>) -> (ResourceSample, Option<CpuTicks>) {
    let current_cpu = host_cpu_ticks();
    let (disk_logical_bytes, disk_allocated_bytes, image_status) = image_allocation(disk_path);
    let memory = host_memory();
    let sample = ResourceSample {
        timestamp_ms: now_ms(),
        scope: "macos.host + harpoon.disk.image",
        cpu_percent: previous_cpu.and_then(|previous| current_cpu.as_ref()?.utilization_since(previous)),
        memory_used_bytes: memory.map(|v| v.0),
        memory_total_bytes: memory.map(|v| v.1),
        disk_logical_bytes,
        disk_allocated_bytes,
        availability: Availability {
            host_cpu: if current_cpu.is_some() && previous_cpu.is_some() { "available" } else { "unavailable:first-sample-or-source" },
            host_memory: if memory.is_some() { "available" } else { "unavailable" },
            image_allocation: image_status,
            disk_activity: "unavailable:no-stable-native-counter",
            vm_guest: "unavailable:no-guest-agent-signal",
            container: "unavailable:not-docker-stats",
        },
    };
    (sample, current_cpu)
}

pub fn start(app: AppHandle, disk_path: Option<String>) -> Result<(), String> {
    stop();
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_stop = stop_flag.clone();
    let path = disk_path.map(PathBuf::from);
    let thread = thread::Builder::new().name("harpoon-resource-sampler".into()).spawn(move || {
        let mut history = Vec::with_capacity(HISTORY_LIMIT);
        let mut previous_cpu = None;
        let mut sample_count = 0;
        while !worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let (sample, current_cpu) = sample(previous_cpu.as_ref(), path.as_ref());
            previous_cpu = current_cpu;
            if history.len() == HISTORY_LIMIT { history.remove(0); }
            history.push(sample);
            sample_count += 1;
            if sample_count % PUBLISH_EVERY == 0 {
                let snapshot = ResourceSnapshot { source: "macos.host", unit: "percent / bytes", samples: history.clone() };
                let _ = app.emit(EVENT, snapshot);
            }
            thread::sleep(SAMPLE_INTERVAL);
        }
    }).map_err(|e| format!("failed to start resource sampler: {e}"))?;
    if let Ok(mut state) = worker_state().lock() { *state = Some(Worker { stop: stop_flag, thread }); }
    Ok(())
}

pub fn stop() {
    let worker = worker_state().lock().ok().and_then(|mut state| state.take());
    if let Some(worker) = worker {
        worker.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = worker.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_delta_is_percentage_and_first_sample_is_missing() {
        let previous = CpuTicks { user: 10, system: 10, idle: 80, nice: 0 };
        assert_eq!(previous.utilization_since(&previous), None);
        let current = CpuTicks { user: 20, system: 20, idle: 90, nice: 0 };
        assert_eq!(current.utilization_since(&previous), Some(66.66666666666666));
        assert_eq!(None::<&CpuTicks>.and_then(|p| current.utilization_since(p)), None);
    }

    #[test]
    fn counter_reset_is_unavailable_not_a_spike() {
        let previous = CpuTicks { user: 100, system: 50, idle: 100, nice: 0 };
        let reset = CpuTicks { user: 1, system: 1, idle: 1, nice: 0 };
        assert_eq!(reset.utilization_since(&previous), None);
    }

    #[test]
    fn history_limit_is_bounded() {
        let mut history = Vec::new();
        for n in 0..(HISTORY_LIMIT + 7) {
            if history.len() == HISTORY_LIMIT { history.remove(0); }
            history.push(n);
        }
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[0], 7);
    }

    #[test]
    fn mac_memory_pages_are_converted_to_nonzero_bytes() {
        assert_eq!(memory_from_page_stats(&[10, 20, 30, 40], 4096, 1_000_000), Some((959_040, 1_000_000)));
    }

    #[test]
    fn mac_memory_missing_or_invalid_source_is_unavailable() {
        assert_eq!(memory_from_page_stats(&[], 4096, 1_000_000), None);
        assert_eq!(memory_from_page_stats(&[10], 0, 1_000_000), None);
        assert_eq!(memory_from_page_stats(&[10], 4096, 0), None);
    }
}
