//! The macOS Keychain as Harpoon's [`SecretStore`] (spec D6, Milestone C).
//!
//! One generic-password item per secret *version*: service `io.harpoon.deploy`, account
//! `<secret-id>@<version>`. Items are written once (`errSecDuplicateItem` is surfaced, never
//! overwritten), read back only inside this process, and never passed through a command line: the
//! value goes to Security.framework as a `CFData`. Every query sets `kSecUseAuthenticationUI` to *fail*,
//! so a locked keychain or an access prompt becomes an error naming the reference instead of a dialog;
//! protection is never weakened to avoid one.
//!
//! No other platform has this store: [`Keychain`] there reports `Unavailable`, and nothing falls back
//! to plaintext.
//!
//! Limitations: the `CFData` copies made by Security.framework are not zeroed by this crate; the value
//! is readable by any process the user's Keychain access control allows (a signed, stable binary
//! identity decides whether macOS prompts: validated against ad-hoc and Developer ID signatures in the
//! Gate U work, not here).

use harpoon_deploy::secrets::{SecretError, SecretRef, SecretStore, SecretValue, SERVICE};

pub struct Keychain {
    service: String,
}

impl Default for Keychain {
    fn default() -> Self {
        Keychain::new()
    }
}

impl Keychain {
    /// The product store: service `io.harpoon.deploy`.
    pub fn new() -> Keychain {
        Keychain {
            service: SERVICE.to_string(),
        }
    }
    /// A different service name, for disposable test items that cannot touch real credentials.
    pub fn with_service(service: &str) -> Keychain {
        Keychain {
            service: service.to_string(),
        }
    }
}

/// Map a Security.framework `OSStatus` to a store error. Pure, so it is tested everywhere.
pub fn map_status(status: i32) -> Result<(), SecretError> {
    match status {
        0 => Ok(()),
        -25300 => Err(SecretError::NotFound),
        -25299 => Err(SecretError::Duplicate),
        -25293 => Err(SecretError::Denied("authorization failed".into())),
        -25308 => Err(SecretError::Denied(
            "the keychain is locked or user interaction is not allowed".into(),
        )),
        -128 => Err(SecretError::Denied("the request was cancelled".into())),
        -25291 | -25294 | -25307 => Err(SecretError::Unavailable("no usable keychain".into())),
        n => Err(SecretError::Other(format!("Security.framework status {n}"))),
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFAllocatorDefault: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        static kCFBooleanTrue: CFTypeRef;
        fn CFStringCreateWithBytes(a: CFTypeRef, b: *const u8, n: CFIndex, enc: u32, ext: u8) -> CFTypeRef;
        fn CFDataCreate(a: CFTypeRef, b: *const u8, n: CFIndex) -> CFTypeRef;
        fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
        fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
        fn CFDictionaryCreateMutable(a: CFTypeRef, cap: CFIndex, k: *const c_void, v: *const c_void) -> CFTypeRef;
        fn CFDictionarySetValue(d: CFTypeRef, k: CFTypeRef, v: CFTypeRef);
        fn CFRelease(r: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: CFTypeRef;
        static kSecClassGenericPassword: CFTypeRef;
        static kSecAttrService: CFTypeRef;
        static kSecAttrAccount: CFTypeRef;
        static kSecValueData: CFTypeRef;
        static kSecReturnData: CFTypeRef;
        static kSecMatchLimit: CFTypeRef;
        static kSecMatchLimitOne: CFTypeRef;
        static kSecUseAuthenticationUI: CFTypeRef;
        static kSecUseAuthenticationUIFail: CFTypeRef;
        fn SecItemAdd(attrs: CFTypeRef, result: *mut CFTypeRef) -> i32;
        fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> i32;
        fn SecItemDelete(query: CFTypeRef) -> i32;
    }

    /// Owns one Core Foundation object and releases it on drop.
    struct Cf(CFTypeRef);
    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was returned by a Create function and is released exactly once.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn string(s: &str) -> Cf {
        // SAFETY: the pointer and length describe `s`; CF copies the bytes.
        Cf(unsafe { CFStringCreateWithBytes(kCFAllocatorDefault, s.as_ptr(), s.len() as CFIndex, UTF8, 0) })
    }

    fn data(b: &[u8]) -> Cf {
        // SAFETY: as above.
        Cf(unsafe { CFDataCreate(kCFAllocatorDefault, b.as_ptr(), b.len() as CFIndex) })
    }

    fn query(service: &str, account: &str, extra: &[(CFTypeRef, CFTypeRef)]) -> Result<(Cf, Vec<Cf>), SecretError> {
        let keep = vec![string(service), string(account)];
        if keep.iter().any(|c| c.0.is_null()) {
            return Err(SecretError::Other("cannot build the keychain query".into()));
        }
        // SAFETY: the callbacks are the documented CF constants; every value outlives the dictionary,
        // which retains what it is given.
        unsafe {
            let d = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                &kCFTypeDictionaryKeyCallBacks as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const c_void,
            );
            if d.is_null() {
                return Err(SecretError::Other("cannot build the keychain query".into()));
            }
            CFDictionarySetValue(d, kSecClass, kSecClassGenericPassword);
            CFDictionarySetValue(d, kSecAttrService, keep[0].0);
            CFDictionarySetValue(d, kSecAttrAccount, keep[1].0);
            // Never show a prompt: a locked keychain becomes an error.
            CFDictionarySetValue(d, kSecUseAuthenticationUI, kSecUseAuthenticationUIFail);
            for (k, v) in extra {
                CFDictionarySetValue(d, *k, *v);
            }
            Ok((Cf(d), keep))
        }
    }

    pub fn put(service: &str, r: &SecretRef, v: &SecretValue) -> Result<(), SecretError> {
        let value = data(v.expose());
        if value.0.is_null() {
            return Err(SecretError::Other("cannot build the keychain item".into()));
        }
        // SAFETY: statics are CF constants; `value` outlives the call.
        let (q, _keep) = query(service, &r.account(), &[unsafe { (kSecValueData, value.0) }])?;
        // SAFETY: `q.0` is a valid dictionary.
        map_status(unsafe { SecItemAdd(q.0, std::ptr::null_mut()) })
    }

    pub fn get(service: &str, r: &SecretRef) -> Result<SecretValue, SecretError> {
        // SAFETY: statics are CF constants.
        let (q, _keep) = query(
            service,
            &r.account(),
            &[unsafe { (kSecReturnData, kCFBooleanTrue) }, unsafe {
                (kSecMatchLimit, kSecMatchLimitOne)
            }],
        )?;
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: `q.0` is valid; `out` receives a CFData we own.
        map_status(unsafe { SecItemCopyMatching(q.0, &mut out) })?;
        let found = Cf(out);
        if found.0.is_null() {
            return Err(SecretError::NotFound);
        }
        // SAFETY: `found.0` is a CFData; the slice is copied before it is released.
        let bytes = unsafe {
            let n = CFDataGetLength(found.0) as usize;
            std::slice::from_raw_parts(CFDataGetBytePtr(found.0), n).to_vec()
        };
        SecretValue::new(bytes).map_err(SecretError::Other)
    }

    pub fn delete(service: &str, r: &SecretRef) -> Result<(), SecretError> {
        let (q, _keep) = query(service, &r.account(), &[])?;
        // SAFETY: `q.0` is a valid dictionary.
        map_status(unsafe { SecItemDelete(q.0) })
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for Keychain {
    fn put(&self, r: &SecretRef, v: &SecretValue) -> Result<(), SecretError> {
        mac::put(&self.service, r, v)
    }
    fn get(&self, r: &SecretRef) -> Result<SecretValue, SecretError> {
        mac::get(&self.service, r)
    }
    fn delete(&self, r: &SecretRef) -> Result<(), SecretError> {
        mac::delete(&self.service, r)
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for Keychain {
    fn put(&self, _: &SecretRef, _: &SecretValue) -> Result<(), SecretError> {
        Err(unavailable())
    }
    fn get(&self, _: &SecretRef) -> Result<SecretValue, SecretError> {
        Err(unavailable())
    }
    fn delete(&self, _: &SecretRef) -> Result<(), SecretError> {
        Err(unavailable())
    }
}

#[cfg(not(target_os = "macos"))]
fn unavailable() -> SecretError {
    SecretError::Unavailable("the macOS Keychain is the only secret store, and this is not macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_errors_that_name_no_value() {
        assert!(map_status(0).is_ok());
        assert_eq!(map_status(-25300), Err(SecretError::NotFound));
        assert_eq!(map_status(-25299), Err(SecretError::Duplicate));
        for denied in [-25293, -25308, -128] {
            assert!(matches!(map_status(denied), Err(SecretError::Denied(_))), "{denied}");
        }
        assert!(matches!(map_status(-25294), Err(SecretError::Unavailable(_))));
        assert!(matches!(map_status(-1), Err(SecretError::Other(_))));
    }

    /// Real Keychain, disposable items only: a unique service name and account, deleted at the end,
    /// so no existing credential can be read, overwritten or removed. `cargo test -- --ignored` on a
    /// Mac with an unlocked login keychain. A locked keychain or a denied prompt makes it fail with
    /// `Denied`, never a dialog.
    #[test]
    #[ignore = "uses the real macOS Keychain (disposable items); run explicitly"]
    fn real_keychain_round_trip_with_disposable_items() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let service = format!("io.harpoon.deploy.test.{}.{nonce}", std::process::id());
        let kc = Keychain::with_service(&service);
        let r = SecretRef::new("test/PLANTED", 1);
        let multi = "-----BEGIN TEST-----\nline two\n-----END TEST-----";
        let v = SecretValue::new(multi.as_bytes().to_vec()).unwrap();
        let result = std::panic::catch_unwind(|| {
            assert_eq!(kc.get(&r).unwrap_err(), SecretError::NotFound);
            kc.put(&r, &v).unwrap();
            assert_eq!(kc.put(&r, &v), Err(SecretError::Duplicate), "versions are immutable");
            assert_eq!(kc.get(&r).unwrap().expose_str(), multi);
            let r2 = SecretRef::new("test/PLANTED", 2);
            assert_eq!(
                kc.get(&r2).unwrap_err(),
                SecretError::NotFound,
                "another version is another item"
            );
        });
        let _ = kc.delete(&r);
        assert_eq!(
            kc.delete(&r),
            Err(SecretError::NotFound),
            "cleaned up, and cleanup is repeatable"
        );
        result.unwrap();
    }
}
