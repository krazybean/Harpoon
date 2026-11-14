//! Read-only telemetry for one container: what Docker's stats endpoint says now, read only after the
//! container is proven to be the one Harpoon recorded. Nothing here starts, stops or changes anything, and
//! nothing reads environment values, command lines or logs. The numbers are Docker's own, converted without
//! inventing: a figure Docker did not provide, or that cannot be computed from one response, is `None`.
//!
//! The parsing is platform-neutral (Docker's API shape, not the host's), so a Linux or Windows adapter can
//! produce the same `Reading` from its own socket.

use crate::api::{ApiError, DockerApi};
use crate::json::Json;
use std::time::Duration;

pub const LABEL_MANAGED: &str = "io.harpoon.managed";
pub const LABEL_APP: &str = "io.harpoon.app";
pub const LABEL_DEPLOYMENT: &str = "io.harpoon.deployment";

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub running: bool,
    /// `State.StartedAt`; it changes at every start, so a different value is a different run.
    pub started_at: String,
    /// `None` when Docker did not report `RestartCount`.
    pub restart_count: Option<u64>,
    pub labels: Vec<(String, String)>,
    /// The limit set on the container (`HostConfig.Memory`), `None` when there is none. Docker's stats `limit`
    /// is the whole VM's memory when no limit is set, which is not a limit of this container.
    pub memory_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sample {
    /// Percent of one core, as `docker stats` shows it: 100 is one core fully used, so it can exceed 100.
    pub cpu_percent: Option<f64>,
    pub cpus: Option<u64>,
    /// `memory_stats.usage`: includes page cache.
    pub mem_raw: Option<u64>,
    /// The page cache Docker reports inside `usage` (inactive file); `None` when it does not report one.
    pub mem_cache: Option<u64>,
    /// Cumulative since the container started, summed over its interfaces.
    pub net_rx: Option<u64>,
    pub net_tx: Option<u64>,
    pub pids: Option<u64>,
}

impl Sample {
    /// `usage` less the reported cache, when Docker reports one. Never guessed.
    pub fn mem_working_set(&self) -> Option<u64> {
        match (self.mem_raw, self.mem_cache) {
            (Some(r), Some(c)) if c <= r => Some(r - c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub identity: Identity,
    /// `None` when the container is not running (there is nothing to measure).
    pub sample: Option<Sample>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Docker has no container by that name or id.
    Absent,
    /// A container with that name exists but is not the one Harpoon recorded; nothing was read from it.
    NotOwned(String),
    Reading(Box<Reading>),
}

fn num(j: Option<&Json>) -> Option<f64> {
    match j {
        Some(Json::Num(n)) if n.is_finite() && *n >= 0.0 => Some(*n),
        _ => None,
    }
}
fn uint(j: Option<&Json>) -> Option<u64> {
    num(j).filter(|n| *n < 9.0e15).map(|n| n as u64)
}

pub fn parse_identity(inspect: &Json) -> Result<Identity, String> {
    let id = inspect.get("Id").and_then(Json::as_str).filter(|s| !s.is_empty()).ok_or("inspect has no Id")?;
    let state = inspect.get("State").ok_or("inspect has no State")?;
    let running = state.get("Running").and_then(Json::as_bool).ok_or("State.Running is not a boolean")?;
    let labels = match inspect.get("Config").and_then(|c| c.get("Labels")) {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Obj(m)) => m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect(),
        Some(_) => return Err("Config.Labels is not an object".into()),
    };
    Ok(Identity {
        id: id.to_string(),
        name: inspect.get("Name").and_then(Json::as_str).unwrap_or_default().trim_start_matches('/').to_string(),
        running,
        started_at: state.get("StartedAt").and_then(Json::as_str).unwrap_or_default().to_string(),
        restart_count: uint(inspect.get("RestartCount")),
        labels,
        memory_limit: uint(inspect.get("HostConfig").and_then(|h| h.get("Memory"))).filter(|m| *m > 0),
    })
}

/// The container must carry Harpoon's labels for this application and deployment, and (when the journal
/// recorded one) its id. A name alone proves nothing: another container can take a name.
pub fn verify_owner(i: &Identity, app: &str, deployment: &str, recorded_id: Option<&str>) -> Result<(), String> {
    let label = |k: &str| i.labels.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if label(LABEL_MANAGED) != Some("true") { return Err(format!("it does not carry {LABEL_MANAGED}=true")); }
    if label(LABEL_APP) != Some(app) { return Err(format!("its {LABEL_APP} label is not this application")); }
    if label(LABEL_DEPLOYMENT) != Some(deployment) { return Err(format!("its {LABEL_DEPLOYMENT} label is not the serving deployment")); }
    if let Some(rec) = recorded_id.filter(|r| !r.is_empty()) {
        let rec = rec.strip_prefix("sha256:").unwrap_or(rec);
        if !(i.id.starts_with(rec) || rec.starts_with(&i.id)) { return Err("its id is not the one recorded for this deployment".into()); }
    }
    Ok(())
}

/// Docker's multicore accounting: the container's CPU time over the system's CPU time across the same interval,
/// times the number of CPUs. Docker supplies the previous sample inside the response (`precpu_stats`); when that
/// half is empty, or a counter went backwards, there is no honest figure.
pub fn cpu_percent(stats: &Json) -> (Option<f64>, Option<u64>) {
    let cur = stats.get("cpu_stats");
    let pre = stats.get("precpu_stats");
    let total = |s: Option<&Json>| uint(s.and_then(|s| s.get("cpu_usage")).and_then(|u| u.get("total_usage")));
    let system = |s: Option<&Json>| uint(s.and_then(|s| s.get("system_cpu_usage")));
    let cpus = uint(cur.and_then(|c| c.get("online_cpus"))).filter(|n| *n > 0).or_else(|| {
        cur.and_then(|c| c.get("cpu_usage")).and_then(|u| u.get("percpu_usage")).and_then(Json::as_array).map(|a| a.len() as u64).filter(|n| *n > 0)
    });
    let pct = (|| {
        let (ct, pt, cs, ps) = (total(cur)?, total(pre)?, system(cur)?, system(pre)?);
        if ps == 0 || cs <= ps || ct < pt { return None; }
        Some((ct - pt) as f64 / (cs - ps) as f64 * cpus? as f64 * 100.0)
    })();
    (pct, cpus)
}

pub fn parse_stats(stats: &Json) -> Sample {
    let mem = stats.get("memory_stats");
    let detail = mem.and_then(|m| m.get("stats"));
    // cgroup v2 reports `inactive_file`, cgroup v1 `total_inactive_file`: the cache `docker stats` subtracts
    let cache = uint(detail.and_then(|d| d.get("inactive_file"))).or_else(|| uint(detail.and_then(|d| d.get("total_inactive_file"))));
    let (cpu_percent, cpus) = cpu_percent(stats);
    let (mut rx, mut tx, mut any) = (0u64, 0u64, false);
    if let Some(Json::Obj(nets)) = stats.get("networks") {
        for (_, n) in nets {
            if let (Some(r), Some(t)) = (uint(n.get("rx_bytes")), uint(n.get("tx_bytes"))) { rx = rx.saturating_add(r); tx = tx.saturating_add(t); any = true; }
        }
    }
    Sample {
        cpu_percent, cpus,
        mem_raw: uint(mem.and_then(|m| m.get("usage"))),
        mem_cache: cache,
        net_rx: any.then_some(rx), net_tx: any.then_some(tx),
        pids: uint(stats.get("pids_stats").and_then(|p| p.get("current"))),
    }
}

/// Resolve, verify, then measure. The stats call uses the verified container's full id, never the name.
pub fn read(api: &DockerApi, name_or_id: &str, app: &str, deployment: &str, recorded_id: Option<&str>, left: Duration) -> Result<Outcome, ApiError> {
    let Some(inspect) = api.container_inspect_json(name_or_id, left)? else { return Ok(Outcome::Absent) };
    let identity = parse_identity(&inspect).map_err(|detail| ApiError::Malformed { op: "container inspect", detail })?;
    if let Err(why) = verify_owner(&identity, app, deployment, recorded_id) { return Ok(Outcome::NotOwned(why)); }
    if !identity.running { return Ok(Outcome::Reading(Box::new(Reading { identity, sample: None }))); }
    let sample = api.container_stats_json(&identity.id, left)?.map(|s| parse_stats(&s));
    Ok(Outcome::Reading(Box::new(Reading { identity, sample })))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn j(t: &str) -> Json { Json::parse(t).unwrap() }
    fn inspect(labels: &str, extra: &str) -> Json {
        j(&format!(r#"{{"Id":"abc123def456","Name":"/hp-dep_1","State":{{"Running":true,"StartedAt":"2026-11-14T00:00:00Z"}},"RestartCount":2,"Config":{{"Labels":{labels}}},"HostConfig":{{"Memory":0}}{extra}}}"#))
    }
    const OWN: &str = r#"{"io.harpoon.managed":"true","io.harpoon.app":"app_1","io.harpoon.deployment":"dep_1"}"#;

    #[test]
    fn a_container_is_ours_only_with_all_three_labels_and_the_recorded_id() {
        let i = parse_identity(&inspect(OWN, "")).unwrap();
        assert_eq!(verify_owner(&i, "app_1", "dep_1", Some("abc123")), Ok(()));
        assert_eq!(verify_owner(&i, "app_1", "dep_1", None), Ok(()));
        assert!(verify_owner(&i, "app_2", "dep_1", None).unwrap_err().contains("application"));
        assert!(verify_owner(&i, "app_1", "dep_2", None).unwrap_err().contains("serving deployment"));
        assert!(verify_owner(&i, "app_1", "dep_1", Some("ffffff")).unwrap_err().contains("recorded"));
        let unmanaged = parse_identity(&inspect(r#"{"io.harpoon.app":"app_1","io.harpoon.deployment":"dep_1"}"#, "")).unwrap();
        assert!(verify_owner(&unmanaged, "app_1", "dep_1", None).is_err());
        let foreign = parse_identity(&inspect("null", "")).unwrap();
        assert!(verify_owner(&foreign, "app_1", "dep_1", None).is_err(), "no labels, no ownership");
    }

    #[test]
    fn identity_reports_the_run_restarts_and_only_a_real_memory_limit() {
        let i = parse_identity(&inspect(OWN, "")).unwrap();
        assert_eq!((i.running, i.restart_count, i.memory_limit, i.name.as_str()), (true, Some(2), None, "hp-dep_1"));
        let limited = parse_identity(&j(r#"{"Id":"a","State":{"Running":false},"HostConfig":{"Memory":268435456}}"#)).unwrap();
        assert_eq!((limited.running, limited.restart_count, limited.memory_limit), (false, None, Some(268435456)));
        assert!(parse_identity(&j(r#"{"Id":"a","State":{"Running":"yes"}}"#)).is_err());
        assert!(parse_identity(&j(r#"{"State":{"Running":true}}"#)).is_err());
    }

    fn stats(cur_total: u64, pre_total: u64, cur_sys: u64, pre_sys: u64, cpus: &str) -> Json {
        j(&format!(r#"{{"cpu_stats":{{"cpu_usage":{{"total_usage":{cur_total}}},"system_cpu_usage":{cur_sys}{cpus}}},"precpu_stats":{{"cpu_usage":{{"total_usage":{pre_total}}},"system_cpu_usage":{pre_sys}}}}}"#))
    }

    #[test]
    fn cpu_follows_dockers_multicore_formula_and_refuses_what_it_cannot_compute() {
        // 50 ms of container CPU over 1 s of 4-CPU system time (4e9 ns): 0.05e9/4e9 * 4 * 100 = 5 %
        let (p, n) = cpu_percent(&stats(150_000_000, 100_000_000, 5_000_000_000, 1_000_000_000, r#","online_cpus":4"#));
        assert_eq!(n, Some(4));
        assert!((p.unwrap() - 5.0).abs() < 1e-9, "{p:?}");
        // a container busy on all four cores is 400 %, not 100 %
        let (full, _) = cpu_percent(&stats(4_000_000_000, 0, 4_000_000_000, 1, r#","online_cpus":4"#));
        assert!((full.unwrap() - 400.0).abs() < 0.001);
        // an idle container is a measured 0, not a missing value
        assert_eq!(cpu_percent(&stats(100, 100, 2_000, 1_000, r#","online_cpus":2"#)).0, Some(0.0));
        // percpu_usage is the fallback for the CPU count
        let old = j(r#"{"cpu_stats":{"cpu_usage":{"total_usage":200,"percpu_usage":[1,1]},"system_cpu_usage":2000},"precpu_stats":{"cpu_usage":{"total_usage":100},"system_cpu_usage":1000}}"#);
        assert_eq!(cpu_percent(&old), (Some(20.0), Some(2)));
        // no previous half (first response), a counter that went backwards, no system time, no CPU count, malformed numbers
        assert_eq!(cpu_percent(&stats(100, 0, 1000, 0, r#","online_cpus":2"#)).0, None, "empty precpu");
        assert_eq!(cpu_percent(&stats(100, 200, 2000, 1000, r#","online_cpus":2"#)).0, None, "counter reset");
        assert_eq!(cpu_percent(&stats(200, 100, 1000, 1000, r#","online_cpus":2"#)).0, None, "no system delta");
        assert_eq!(cpu_percent(&stats(200, 100, 2000, 1000, "")).0, None, "unknown CPU count");
        assert_eq!(cpu_percent(&j(r#"{"cpu_stats":{"cpu_usage":{"total_usage":"x"}}}"#)).0, None);
        assert_eq!(cpu_percent(&j("{}")).0, None);
    }

    #[test]
    fn memory_distinguishes_raw_usage_from_the_cache_docker_reports() {
        let v2 = parse_stats(&j(r#"{"memory_stats":{"usage":1000,"limit":99999,"stats":{"inactive_file":300}}}"#));
        assert_eq!((v2.mem_raw, v2.mem_cache, v2.mem_working_set()), (Some(1000), Some(300), Some(700)));
        let v1 = parse_stats(&j(r#"{"memory_stats":{"usage":1000,"stats":{"total_inactive_file":100}}}"#));
        assert_eq!(v1.mem_working_set(), Some(900));
        let none = parse_stats(&j(r#"{"memory_stats":{"usage":1000,"stats":{}}}"#));
        assert_eq!((none.mem_raw, none.mem_cache, none.mem_working_set()), (Some(1000), None, None), "no cache figure, no adjusted figure");
        let odd = parse_stats(&j(r#"{"memory_stats":{"usage":100,"stats":{"inactive_file":500}}}"#));
        assert_eq!(odd.mem_working_set(), None, "a cache larger than usage is inconsistent, so nothing is derived");
        assert_eq!(parse_stats(&j("{}")).mem_raw, None);
    }

    #[test]
    fn network_counters_are_summed_across_interfaces_and_absent_when_missing() {
        let s = parse_stats(&j(r#"{"networks":{"eth0":{"rx_bytes":100,"tx_bytes":10},"eth1":{"rx_bytes":5,"tx_bytes":1}},"pids_stats":{"current":7}}"#));
        assert_eq!((s.net_rx, s.net_tx, s.pids), (Some(105), Some(11), Some(7)));
        let zero = parse_stats(&j(r#"{"networks":{"eth0":{"rx_bytes":0,"tx_bytes":0}}}"#));
        assert_eq!((zero.net_rx, zero.net_tx), (Some(0), Some(0)), "a measured zero stays zero");
        for bad in [r#"{}"#, r#"{"networks":{}}"#, r#"{"networks":{"eth0":{"rx_bytes":"x","tx_bytes":1}}}"#, r#"{"networks":[1]}"#, r#"{"networks":{"eth0":{"rx_bytes":-5,"tx_bytes":1}}}"#] {
            let s = parse_stats(&j(bad));
            assert_eq!((s.net_rx, s.net_tx), (None, None), "{bad}");
        }
    }
}
