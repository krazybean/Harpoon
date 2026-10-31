//! The immutable, non-secret configuration needed to recreate a release (spec D5, cutover
//! recovery). It is pinned when a release starts serving and stored with the deployment's durable
//! snapshot. It holds no environment values and no secrets: the per-container response marker is
//! generated afresh for every container, and secrets are not part of this milestone.

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
    /// One line without tabs or newlines: `v1;key=value;…` with free text hex-encoded.
    pub fn encode(&self) -> String {
        let expect = match self.health.expect {
            StatusClass::Success2xx => "2xx",
            StatusClass::NotServerError => "lt500",
        };
        format!(
            "v1;img={};tag={};cport={};hport={};path={};expect={expect};interval={};ok={};deadline={};mem={};cpu={};pids={}",
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
        )
    }

    pub fn decode(s: &str) -> Result<ReleaseConfig, String> {
        let mut parts = s.split(';');
        if parts.next() != Some("v1") {
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
        let tag_id = c.encode().replace(&hex(&c.image_id), &hex("latest"));
        assert!(
            ReleaseConfig::decode(&tag_id).is_err(),
            "a tag is not an exact image id"
        );
    }
}
