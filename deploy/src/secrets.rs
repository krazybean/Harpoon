//! Secret values and where they live (spec D6, Milestone C).
//!
//! A secret is identified by a [`SecretRef`] (an id and an exact, immutable version). The journal
//! stores only references; the value lives in a [`SecretStore`] (the macOS Keychain in the product, an
//! in-memory fake in tests). A value is held in a [`SecretValue`], which has no `Display`, a redacted
//! `Debug`, and is zeroed on drop (best effort), so formatting an error or a panic cannot print it.
//!
//! Nothing here falls back to plaintext: a store that cannot answer is an error naming the reference,
//! never the value.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// Keychain service name for every Harpoon deployment secret.
pub const SERVICE: &str = "io.harpoon.deploy";
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// One immutable saved version of a secret. `account()` is the Keychain account string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef {
    pub id: String,
    pub version: u32,
}

impl SecretRef {
    pub fn new(id: impl Into<String>, version: u32) -> SecretRef {
        SecretRef { id: id.into(), version }
    }
    /// `<secret-id>@<version>`, the Keychain account (D6).
    pub fn account(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
    pub fn parse(s: &str) -> Result<SecretRef, String> {
        let (id, v) = s.rsplit_once('@').ok_or("a secret reference is <id>@<version>")?;
        let version: u32 = v.parse().map_err(|_| "the secret version is not a number")?;
        if id.is_empty() || version == 0 {
            return Err("a secret reference needs an id and a version of at least 1".into());
        }
        Ok(SecretRef {
            id: id.to_string(),
            version,
        })
    }
}

impl std::fmt::Display for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

/// A secret value. Deliberately has no `Display`, `Clone`-by-accident formatting or serialization.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    /// Validates: non-empty, at most [`MAX_SECRET_BYTES`], UTF-8, no NUL. Newlines are allowed
    /// (multi-line secrets such as a PEM key are real). The error never contains the value.
    pub fn new(bytes: Vec<u8>) -> Result<SecretValue, String> {
        if bytes.is_empty() {
            return Err("a secret value cannot be empty".into());
        }
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(format!("a secret value is at most {MAX_SECRET_BYTES} bytes"));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err("a secret value must be UTF-8 text".into());
        }
        if bytes.contains(&0) {
            return Err("a secret value cannot contain a NUL byte".into());
        }
        Ok(SecretValue(bytes))
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
    pub fn expose_str(&self) -> &str {
        // Validated as UTF-8 at construction.
        std::str::from_utf8(&self.0).unwrap_or("")
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> SecretValue {
        SecretValue(self.0.clone())
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        // ponytail: best-effort zeroing (volatile so the compiler keeps it); not a guarantee against
        // copies the allocator or the OS made. Upgrade path: a `zeroize`-style crate if one is ever allowed.
        for b in self.0.iter_mut() {
            // SAFETY: `b` is a valid, exclusive reference.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// No item for that reference.
    NotFound,
    /// The store refused: locked, access denied, user cancelled, interaction not allowed.
    Denied(String),
    /// The store itself could not be reached (not macOS, no keychain).
    Unavailable(String),
    /// Saving would overwrite an existing version (versions are immutable).
    Duplicate,
    Other(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::NotFound => f.write_str("not found in the secret store"),
            SecretError::Denied(m) => write!(f, "the secret store refused access ({m})"),
            SecretError::Unavailable(m) => write!(f, "the secret store is unavailable ({m})"),
            SecretError::Duplicate => f.write_str("that secret version already exists (versions are immutable)"),
            SecretError::Other(m) => write!(f, "the secret store failed ({m})"),
        }
    }
}

/// The storage seam. The product implements it with the macOS Keychain; tests use
/// [`MemorySecretStore`]. Implementations must never put a value in an error.
pub trait SecretStore {
    /// Save one immutable version. `Duplicate` if it already exists.
    fn put(&self, r: &SecretRef, v: &SecretValue) -> Result<(), SecretError>;
    fn get(&self, r: &SecretRef) -> Result<SecretValue, SecretError>;
    /// Remove one version (used to undo a save whose record could not be written, and by tests).
    fn delete(&self, r: &SecretRef) -> Result<(), SecretError>;
}

/// Resolved values for the references one operation needs, held in memory only.
#[derive(Default)]
pub struct SecretBundle {
    values: BTreeMap<SecretRef, SecretValue>,
}

impl std::fmt::Debug for SecretBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBundle({} values, <redacted>)", self.values.len())
    }
}

impl SecretBundle {
    pub fn new() -> SecretBundle {
        SecretBundle::default()
    }
    pub fn insert(&mut self, r: SecretRef, v: SecretValue) {
        self.values.insert(r, v);
    }
    pub fn get(&self, r: &SecretRef) -> Option<&SecretValue> {
        self.values.get(r)
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub fn values(&self) -> impl Iterator<Item = &SecretValue> {
        self.values.values()
    }
}

/// What an in-memory store should do next, to test locked / denied / unavailable behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeMode {
    Working,
    Locked,
    Denied,
    Unavailable,
}

/// Deterministic fake for tests. Public (not `cfg(test)`) because other crates' tests use it.
pub struct MemorySecretStore {
    items: Mutex<BTreeMap<String, Vec<u8>>>,
    mode: Mutex<FakeMode>,
    reads: Mutex<usize>,
}

impl Default for MemorySecretStore {
    fn default() -> Self {
        MemorySecretStore::new()
    }
}

impl MemorySecretStore {
    pub fn new() -> MemorySecretStore {
        MemorySecretStore {
            items: Mutex::new(BTreeMap::new()),
            mode: Mutex::new(FakeMode::Working),
            reads: Mutex::new(0),
        }
    }
    pub fn set_mode(&self, m: FakeMode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = m;
    }
    pub fn reads(&self) -> usize {
        *self.reads.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Every stored byte string, for tests that assert what the store holds.
    pub fn accounts(&self) -> Vec<String> {
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }
    fn check(&self) -> Result<(), SecretError> {
        match *self.mode.lock().unwrap_or_else(|e| e.into_inner()) {
            FakeMode::Working => Ok(()),
            FakeMode::Locked => Err(SecretError::Denied("the keychain is locked".into())),
            FakeMode::Denied => Err(SecretError::Denied("access denied".into())),
            FakeMode::Unavailable => Err(SecretError::Unavailable("no keychain".into())),
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, r: &SecretRef, v: &SecretValue) -> Result<(), SecretError> {
        self.check()?;
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        if items.contains_key(&r.account()) {
            return Err(SecretError::Duplicate);
        }
        items.insert(r.account(), v.expose().to_vec());
        Ok(())
    }
    fn get(&self, r: &SecretRef) -> Result<SecretValue, SecretError> {
        self.check()?;
        *self.reads.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let bytes = items.get(&r.account()).ok_or(SecretError::NotFound)?;
        SecretValue::new(bytes.clone()).map_err(SecretError::Other)
    }
    fn delete(&self, r: &SecretRef) -> Result<(), SecretError> {
        self.check()?;
        self.items
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&r.account())
            .map(|_| ())
            .ok_or(SecretError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_never_format_and_validate_without_echoing() {
        let v = SecretValue::new(b"hunter2".to_vec()).unwrap();
        assert!(!format!("{v:?}").contains("hunter2"));
        let b = {
            let mut b = SecretBundle::new();
            b.insert(SecretRef::new("s", 1), v.clone());
            b
        };
        assert!(!format!("{b:?}").contains("hunter2"));
        for bad in [
            vec![],
            vec![0xff, 0xfe],
            b"a\0b".to_vec(),
            vec![b'x'; MAX_SECRET_BYTES + 1],
        ] {
            let e = SecretValue::new(bad).unwrap_err();
            assert!(!e.contains("xxxx"));
        }
        assert!(SecretValue::new(b"line1\nline2\n".to_vec()).is_ok());
        assert!(SecretValue::new(b"x".to_vec()).is_ok(), "no minimum length");
    }

    #[test]
    fn references_round_trip_and_the_fake_store_is_immutable_and_can_fail() {
        let r = SecretRef::new("app:a/DB", 3);
        assert_eq!(SecretRef::parse(&r.to_string()).unwrap(), r);
        assert!(SecretRef::parse("x@0").is_err() && SecretRef::parse("@1").is_err() && SecretRef::parse("x").is_err());
        let s = MemorySecretStore::new();
        let v = SecretValue::new(b"v".to_vec()).unwrap();
        s.put(&r, &v).unwrap();
        assert_eq!(s.put(&r, &v), Err(SecretError::Duplicate));
        assert_eq!(s.get(&r).unwrap().expose(), b"v");
        assert_eq!(
            s.get(&SecretRef::new("app:a/DB", 4)).unwrap_err(),
            SecretError::NotFound
        );
        s.set_mode(FakeMode::Locked);
        assert!(matches!(s.get(&r), Err(SecretError::Denied(_))));
    }
}
