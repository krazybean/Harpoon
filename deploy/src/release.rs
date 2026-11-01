//! The immutable, non-secret configuration needed to recreate a release (spec D5, cutover
//! recovery). It is pinned when a release starts serving and stored with the deployment's durable
//! snapshot. It holds the release's **non-secret** environment variables (`--env`, see
//! [`crate::envvars`]) and no secrets: the values are visible in `docker inspect` and in the journal.
//! The per-container response marker is not part of it; it is generated afresh for every container.

use crate::health::{HealthSpec, StatusClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseConfig {
    /// Exact identity of the image (`sha256:…`), never a tag that can move.
    pub image_id: String,
    /// The tag the image was built under, for display.
    pub image_tag: String,
    pub container_port: u16,
    /// The published host port the release served on.
    pub host_port: u16,
    pub health: HealthSpec,
    pub memory_bytes: i64,
    pub nano_cpus: i64,
    pub pids: i64,
    /// The release's non-secret environment, sorted by name. Empty for a release that had none
    /// and for every release pinned before this field existed.
    pub env: Vec<(String, String)>,
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

impl ReleaseConfig {
    /// One line without tabs or newlines: `v1;key=value;…` with free text hex-encoded. A release
    /// with no environment encodes exactly as before (`v1`); one with an environment adds a trailing
    /// `env=` field and is `v2`. Older readers fail closed on `v2` rather than dropping the variables.
    pub fn encode(&self) -> String {
        let expect = match self.health.expect {
            StatusClass::Success2xx => "2xx",
            StatusClass::NotServerError => "lt500",
        };
        let base = format!(
            "img={};tag={};cport={};hport={};path={};expect={expect};interval={};ok={};deadline={};mem={};cpu={};pids={}",
            hex(&self.image_id),
            hex(&self.image_tag),
            self.container_port,
            self.host_port,
            hex(&self.health.path),
            self.health.interval_ms,
            self.health.consecutive_ok,
            self.health.startup_deadline_ms,
            self.memory_bytes,
            self.nano_cpus,
            self.pids,
        );
        if self.env.is_empty() {
            format!("v1;{base}")
        } else {
            let env: Vec<String> = self.env.iter().map(|(k, v)| hex(&format!("{k}={v}"))).collect();
            format!("v2;{base};env={}", env.join(","))
        }
    }

    pub fn decode(s: &str) -> Result<ReleaseConfig, String> {
        let mut parts = s.split(';');
        let version = parts.next();
        if version != Some("v1") && version != Some("v2") {
            return Err("unknown pinned-configuration version".into());
        }
        let mut get = |key: &str| -> Result<&str, String> {
            let p = parts.next().ok_or_else(|| format!("missing {key}"))?;
            p.strip_prefix(&format!("{key}="))
                .ok_or_else(|| format!("expected {key}"))
        };
        let num =
            |v: &str, what: &str| -> Result<i64, String> { v.parse().map_err(|_| format!("{what} is not a number")) };
        let text = |v: &str, what: &str| unhex(v).ok_or_else(|| format!("{what} is not valid"));
        let image_id = text(get("img")?, "image id")?;
        let image_tag = text(get("tag")?, "image tag")?;
        let container_port = num(get("cport")?, "container port")?;
        let host_port = num(get("hport")?, "host port")?;
        let path = text(get("path")?, "health path")?;
        let expect = match get("expect")? {
            "2xx" => StatusClass::Success2xx,
            "lt500" => StatusClass::NotServerError,
            other => return Err(format!("unknown health expectation {other:?}")),
        };
        let interval = num(get("interval")?, "health interval")?;
        let ok = num(get("ok")?, "consecutive successes")?;
        let deadline = num(get("deadline")?, "health deadline")?;
        let (memory_bytes, nano_cpus, pids) = (
            num(get("mem")?, "memory")?,
            num(get("cpu")?, "cpu")?,
            num(get("pids")?, "pids")?,
        );
        let env = if version == Some("v2") {
            let raw = get("env")?;
            if raw.is_empty() {
                return Err("a v2 pinned configuration has an empty environment (it would be v1)".into());
            }
            let mut pairs = Vec::new();
            for item in raw.split(',') {
                let kv = unhex(item).ok_or("an environment entry is not valid")?;
                let (k, v) = kv.split_once('=').ok_or("an environment entry has no name")?;
                pairs.push((k.to_string(), v.to_string()));
            }
            let canonical = crate::envvars::validate(&pairs).map_err(|e| format!("pinned environment: {e}"))?;
            if canonical != pairs {
                return Err("the pinned environment is not in canonical order".into());
            }
            canonical
        } else {
            Vec::new()
        };
        if parts.next().is_some() {
            return Err("unexpected trailing fields".into());
        }
        let port = |v: i64, what: &str| u16::try_from(v).map_err(|_| format!("{what} is out of range"));
        if !image_id.starts_with("sha256:") {
            return Err("the pinned image is not an exact image id".into());
        }
        Ok(ReleaseConfig {
            image_id,
            image_tag,
            container_port: port(container_port, "container port")?,
            host_port: port(host_port, "host port")?,
            health: HealthSpec {
                path,
                expect,
                interval_ms: u64::try_from(interval).map_err(|_| "health interval is negative")?,
                consecutive_ok: u8::try_from(ok).map_err(|_| "consecutive successes out of range")?,
                startup_deadline_ms: u64::try_from(deadline).map_err(|_| "health deadline is negative")?,
            },
            memory_bytes,
            nano_cpus,
            pids,
            env,
        })
    }

    /// A pin for fakes and offline simulation. Names nothing real.
    pub fn simulated(host_port: u16) -> ReleaseConfig {
        ReleaseConfig {
            image_id: format!("sha256:{}", "0".repeat(64)),
            image_tag: "simulated".into(),
            container_port: 8080,
            host_port,
            health: HealthSpec::first_test("/"),
            memory_bytes: 256 * 1024 * 1024,
            nano_cpus: 500_000_000,
            pids: 64,
            env: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_damage() {
        let mut c = ReleaseConfig::simulated(38007);
        c.health.path = "/h;ealth=1\t\n".into();
        assert_eq!(ReleaseConfig::decode(&c.encode()).unwrap(), c);
        assert!(!c.encode().contains(['\t', '\n']));
        for bad in [
            "",
            "v2;x",
            "v1;img=zz",
            &c.encode().replace("hport=38007", "hport=99999"),
        ] {
            assert!(ReleaseConfig::decode(bad).is_err(), "{bad}");
        }
        assert!(ReleaseConfig::decode(&format!("{};extra=1", c.encode())).is_err());
        let mut e = c.clone();
        // values that stress the field and entry separators
        e.env = vec![("A".into(), "".into()), ("B".into(), "x=y;z,w".into())];
        let enc = e.encode();
        assert!(enc.starts_with("v2;") && !enc.contains(['\t', '\n']), "{enc}");
        assert_eq!(ReleaseConfig::decode(&enc).unwrap(), e);
        assert!(c.encode().starts_with("v1;"), "no environment keeps the legacy form");
        assert!(
            ReleaseConfig::decode(&enc.replace("v2;", "v1;")).is_err(),
            "v1 has no env field"
        );
        assert!(ReleaseConfig::decode(&format!("{};env=", c.encode().replace("v1;", "v2;"))).is_err());
        let out_of_order = format!("{};env={},{}", c.encode().replace("v1;", "v2;"), hex("B=1"), hex("A=1"));
        assert!(ReleaseConfig::decode(&out_of_order).is_err());
        let reserved = format!(
            "{};env={}",
            c.encode().replace("v1;", "v2;"),
            hex("HARPOON_DEPLOYMENT_MARKER=x")
        );
        assert!(ReleaseConfig::decode(&reserved).is_err());
        let tag_id = c.encode().replace(&hex(&c.image_id), &hex("latest"));
        assert!(
            ReleaseConfig::decode(&tag_id).is_err(),
            "a tag is not an exact image id"
        );
    }

    /// A pin written by the build that predates `env` (taken from a real journal): it decodes with an
    /// empty environment and encodes back to exactly the same text, so old snapshots never change.
    #[test]
    fn a_pin_from_before_the_environment_existed_decodes_and_re_encodes_unchanged() {
        let legacy = "v1;img=7368613235363a36383738303733393738313035613637303439363031623233386331366132656634653135363265663162613633306365363237306430383839323234323166;tag=686172706f6f6e2f6170705f30316d3661683763316867323168376e783235383830686132633a6465705f30314d3641483835473948523745395248374d323535535350342d3038313932393261343663356638633165393332653633363432396230376330;cport=8080;hport=38002;path=2f6865616c7468;expect=2xx;interval=500;ok=2;deadline=60000;mem=268435456;cpu=500000000;pids=64";
        let c = ReleaseConfig::decode(legacy).unwrap();
        assert!(c.env.is_empty());
        assert_eq!(c.host_port, 38002);
        assert_eq!(c.encode(), legacy);
    }
}
