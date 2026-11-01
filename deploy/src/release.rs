//! The immutable configuration needed to recreate a release (spec D5, cutover recovery). It is
//! pinned when a release starts serving and stored with the deployment's durable snapshot. It holds
//! the release's **non-secret** environment (see [`crate::envvars`]: visible in `docker inspect` and
//! in the journal), the **references** (id and exact version) of its runtime secrets, the identity of
//! the configuration snapshot it came from, and the identity of its build configuration. It never
//! holds a secret value: a restore or rollback resolves the pinned versions from the secret store.
//! The per-container response marker is not part of it; it is generated afresh for every container.

use crate::health::{HealthSpec, StatusClass};
use crate::secrets::SecretRef;

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
    /// The release's runtime secrets: environment name and the exact saved version, sorted by name.
    pub secrets: Vec<(String, SecretRef)>,
    /// The deployment whose configuration snapshot (provenance of every value) this release came
    /// from. A rollback reuses its target's, never the current command's.
    pub snapshot: Option<String>,
    /// Canonical description of the build configuration (Dockerfile, target stage, build arguments,
    /// build-secret references): what an unchanged image depends on. No values of secrets.
    pub build: Option<String>,
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
    /// `env=` field and is `v2`; one with secret references, a snapshot or a build identity is `v3`
    /// (`env=;sec=;snap=;build=`, each possibly empty). Older readers fail closed on `v2` and `v3`
    /// rather than dropping what they do not understand.
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
        let env: Vec<String> = self.env.iter().map(|(k, v)| hex(&format!("{k}={v}"))).collect();
        if self.secrets.is_empty() && self.snapshot.is_none() && self.build.is_none() {
            if self.env.is_empty() {
                format!("v1;{base}")
            } else {
                format!("v2;{base};env={}", env.join(","))
            }
        } else {
            let sec: Vec<String> = self.secrets.iter().map(|(n, r)| hex(&format!("{n}={r}"))).collect();
            format!(
                "v3;{base};env={};sec={};snap={};build={}",
                env.join(","),
                sec.join(","),
                self.snapshot.as_deref().map(hex).unwrap_or_default(),
                self.build.as_deref().map(hex).unwrap_or_default()
            )
        }
    }

    pub fn decode(s: &str) -> Result<ReleaseConfig, String> {
        let mut parts = s.split(';');
        let version = parts.next();
        if version != Some("v1") && version != Some("v2") && version != Some("v3") {
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
        let env = if version == Some("v2") || version == Some("v3") {
            let raw = get("env")?;
            if raw.is_empty() && version == Some("v2") {
                return Err("a v2 pinned configuration has an empty environment (it would be v1)".into());
            }
            let mut pairs = Vec::new();
            for item in raw.split(',').filter(|i| !i.is_empty()) {
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
        let (mut secrets, mut snapshot, mut build) = (Vec::new(), None, None);
        if version == Some("v3") {
            let raw = get("sec")?;
            for item in raw.split(',').filter(|i| !i.is_empty()) {
                let kv = unhex(item).ok_or("a secret entry is not valid")?;
                let (n, r) = kv.split_once('=').ok_or("a secret entry has no name")?;
                crate::envvars::check_name(n).map_err(|e| format!("pinned secret: {e}"))?;
                secrets.push((
                    n.to_string(),
                    SecretRef::parse(r).map_err(|e| format!("pinned secret: {e}"))?,
                ));
            }
            let names: Vec<&String> = secrets.iter().map(|(n, _)| n).collect();
            if names.windows(2).any(|w| w[0] >= w[1]) {
                return Err("the pinned secrets are not unique and in canonical order".into());
            }
            if secrets.iter().any(|(n, _)| env.iter().any(|(k, _)| k == n)) {
                return Err("a name is pinned both as a value and as a secret".into());
            }
            let opt = |v: &str, what: &str| -> Result<Option<String>, String> {
                if v.is_empty() {
                    Ok(None)
                } else {
                    unhex(v)
                        .map(Some)
                        .ok_or_else(|| format!("the pinned {what} is not valid"))
                }
            };
            snapshot = opt(get("snap")?, "snapshot id")?;
            build = opt(get("build")?, "build identity")?;
            if secrets.is_empty() && snapshot.is_none() && build.is_none() {
                return Err("a v3 pinned configuration with nothing v3 about it (it would be v1 or v2)".into());
            }
        }
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
            secrets,
            snapshot,
            build,
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
            secrets: Vec::new(),
            snapshot: None,
            build: None,
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

    #[test]
    fn v3_carries_secret_references_snapshot_and_build_identity_and_never_a_value() {
        let mut c = ReleaseConfig::simulated(38007);
        c.env = vec![("A".into(), "1".into())];
        c.secrets = vec![
            (
                "DB".into(),
                SecretRef::new("app:app_00000000000000000000000001@prod/DB", 3),
            ),
            ("TOKEN".into(), SecretRef::new("global/TOKEN", 1)),
        ];
        c.snapshot = Some("dep_00000000000000000000000010".into());
        c.build = Some("target=release;args=A=1".into());
        let enc = c.encode();
        assert!(enc.starts_with("v3;") && !enc.contains(['\t', '\n']), "{enc}");
        assert_eq!(ReleaseConfig::decode(&enc).unwrap(), c);
        // secrets only, no environment
        let mut s = ReleaseConfig::simulated(1);
        s.secrets = vec![("X".into(), SecretRef::new("g/X", 2))];
        assert_eq!(ReleaseConfig::decode(&s.encode()).unwrap(), s);
        // v1 and v2 still decode and re-encode unchanged next to it
        for older in [ReleaseConfig::simulated(2), {
            let mut v = ReleaseConfig::simulated(3);
            v.env = vec![("A".into(), "1".into())];
            v
        }] {
            assert_eq!(ReleaseConfig::decode(&older.encode()).unwrap(), older);
        }
        // damage: out of order, duplicate, overlap with env, empty v3, bad reference
        let sec = |items: &[(&str, &str)]| {
            let l: Vec<String> = items.iter().map(|(n, r)| hex(&format!("{n}={r}"))).collect();
            format!(
                "{};env=;sec={};snap=;build=",
                ReleaseConfig::simulated(1).encode().replace("v1;", "v3;"),
                l.join(",")
            )
        };
        assert!(ReleaseConfig::decode(&sec(&[("B", "g/B@1"), ("A", "g/A@1")])).is_err());
        assert!(ReleaseConfig::decode(&sec(&[("A", "g/A@1"), ("A", "g/A@2")])).is_err());
        assert!(ReleaseConfig::decode(&sec(&[("A", "g/A@0")])).is_err());
        assert!(ReleaseConfig::decode(&sec(&[("HARPOON_X", "g/A@1")])).is_err());
        assert!(ReleaseConfig::decode(&sec(&[])).is_err(), "v3 with nothing in it");
        assert!(ReleaseConfig::decode(&sec(&[("A", "g/A@1")])).is_ok());
        let overlap = format!(
            "{};env={};sec={};snap=;build=",
            ReleaseConfig::simulated(1).encode().replace("v1;", "v3;"),
            hex("A=1"),
            hex("A=g/A@1")
        );
        assert!(ReleaseConfig::decode(&overlap).is_err());
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
