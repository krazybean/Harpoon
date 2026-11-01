//! Non-secret runtime environment variables for a release (`--env KEY=VALUE`).
//!
//! These are **not secrets**. They are passed to the container, so `docker inspect` shows them, and
//! they are pinned in the deployment's durable snapshot (SQLite), so anyone who can read the journal
//! can read them. Secret handling (Keychain, redaction) belongs to Milestone C; nothing here protects
//! a value. Validation happens before any Docker mutation or journal write.

use std::collections::BTreeSet;

/// The per-container response marker Harpoon generates itself; it can never be configured.
pub const MARKER: &str = "HARPOON_DEPLOYMENT_MARKER";
/// Every name starting with this is reserved for Harpoon-managed identity.
pub const RESERVED_PREFIX: &str = "HARPOON_";
pub const MAX_VARIABLES: usize = 64;
pub const MAX_NAME_BYTES: usize = 128;
pub const MAX_VALUE_BYTES: usize = 4096;
pub const MAX_TOTAL_BYTES: usize = 32 * 1024;

/// One validated `KEY=VALUE` from the command line: the name is everything before the first `=`,
/// the value everything after it (it may be empty and may contain `=`).
pub fn parse_one(arg: &str) -> Result<(String, String), String> {
    let (name, value) = arg
        .split_once('=')
        .ok_or_else(|| format!("--env {arg:?} must look like KEY=VALUE (use KEY= for an explicit empty value)"))?;
    check_name(name)?;
    check_value(name, value)?;
    Ok((name.to_string(), value.to_string()))
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("an --env name is empty".into());
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(format!("--env name {name:?} is longer than {MAX_NAME_BYTES} bytes"));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('_');
    if !(first.is_ascii_alphabetic() || first == '_') || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "--env name {name:?} is not valid: use letters, digits and underscores, not starting with a digit"
        ));
    }
    if name.starts_with(RESERVED_PREFIX) {
        return Err(format!(
            "--env name {name:?} is reserved: names starting with {RESERVED_PREFIX} belong to Harpoon{}",
            if name == MARKER {
                " (the per-container response marker is generated for every container and cannot be set)"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

fn check_value(name: &str, value: &str) -> Result<(), String> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(format!(
            "the value of --env {name} is longer than {MAX_VALUE_BYTES} bytes"
        ));
    }
    if let Some(c) = value.chars().find(|c| c.is_control()) {
        return Err(format!(
            "the value of --env {name} contains a control character (U+{:04X}); only printable text is accepted",
            c as u32
        ));
    }
    Ok(())
}

/// Validate a whole set (also used on a decoded pin): unique valid names, bounded size. Returns it
/// sorted by name, which is the canonical order everywhere (pin, container, display).
pub fn validate(pairs: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
    if pairs.len() > MAX_VARIABLES {
        return Err(format!(
            "{} variables given; at most {MAX_VARIABLES} are accepted",
            pairs.len()
        ));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    for (n, v) in pairs {
        check_name(n)?;
        check_value(n, v)?;
        if !seen.insert(n.clone()) {
            return Err(format!("--env {n} is given more than once"));
        }
        total += n.len() + v.len() + 1;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(format!(
            "the variables total {total} bytes; at most {MAX_TOTAL_BYTES} are accepted"
        ));
    }
    let mut out = pairs.to_vec();
    out.sort();
    Ok(out)
}

/// Parse every `--env` argument of a command.
pub fn parse_all<S: AsRef<str>>(args: &[S]) -> Result<Vec<(String, String)>, String> {
    let pairs = args
        .iter()
        .map(|a| parse_one(a.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    validate(&pairs)
}

/// `KEY=VALUE` strings as Docker takes them.
pub fn docker_form(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Names only, for display. Values are never printed by Harpoon's own output.
pub fn names(pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        "none".into()
    } else {
        pairs.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Vec<(String, String)>, String> {
        parse_all(a)
    }

    #[test]
    fn keeps_equals_signs_and_explicit_empty_values_and_sorts() {
        let v = p(&["B=x=y=z", "A=", "C=with space and = sign"]).unwrap();
        assert_eq!(v[0], ("A".into(), "".into()));
        assert_eq!(v[1], ("B".into(), "x=y=z".into()));
        assert_eq!(v[2].1, "with space and = sign");
    }

    #[test]
    fn rejects_malformed_duplicate_reserved_and_unsafe() {
        for bad in [
            "NOEQUALS",
            "=v",
            "1A=v",
            "A-B=v",
            "a b=v",
            "A=line\nbreak",
            "A=nul\0",
            "A=tab\t",
            "A=\u{7f}",
            "HARPOON_X=1",
            "HARPOON_DEPLOYMENT_MARKER=abc",
            "é=v",
        ] {
            assert!(p(&[bad]).is_err(), "{bad:?}");
        }
        assert!(p(&["A=1", "A=2"]).unwrap_err().contains("more than once"));
        assert!(p(&["HARPOON_DEPLOYMENT_MARKER=x"]).unwrap_err().contains("marker"));
        assert!(parse_one(&format!("A={}", "x".repeat(MAX_VALUE_BYTES + 1))).is_err());
        let many: Vec<String> = (0..=MAX_VARIABLES).map(|i| format!("V{i}=1")).collect();
        assert!(parse_all(&many).is_err());
    }

    #[test]
    fn display_shows_names_only() {
        let v = p(&["B=secret-looking", "A=1"]).unwrap();
        assert_eq!(names(&v), "A, B");
        assert!(!names(&v).contains("secret"));
        assert_eq!(names(&[]), "none");
    }
}
