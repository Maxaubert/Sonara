//! Keys of external engines (spec section 6): Windows Credential Manager
//! (target `sonara:<profile id>`) or an environment variable the profile
//! names. A key is never in a file of the home (the `FileStore` is a
//! testing aid for `sonarad --keys fake`), never logged, never in a reply.
use super::error::ExtError;
use super::profile::{KeyRef, Profile};
use crate::Reason;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The longest key Credential Manager stores (CRED_MAX_CREDENTIAL_BLOB_SIZE).
pub const MAX_KEY_BYTES: usize = 2560;
/// Credential target prefix.
pub const TARGET_PREFIX: &str = "sonara:";

/// A secret value. `Debug` and `Display` print `[redacted]`, there is no
/// `Serialize`, and the buffer is overwritten with zeros on drop (best
/// effort).
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Secret {
        Secret(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Clone for Secret {
    fn clone(&self) -> Self {
        Secret(self.0.clone())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: zero bytes keep the string valid UTF-8.
        let bytes = unsafe { self.0.as_mut_vec() };
        for b in bytes.iter_mut() {
            // SAFETY: a valid, aligned pointer into the vector.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

/// Why a key store failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// No Credential Manager in this build (not Windows).
    Unavailable,
    TooLong,
    Os(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Unavailable => f.write_str("Credential Manager is not available"),
            KeyError::TooLong => write!(f, "key too long (at most {MAX_KEY_BYTES} bytes)"),
            KeyError::Os(m) => write!(f, "Credential Manager: {m}"),
        }
    }
}

/// Where keys are kept, by profile id.
pub trait KeyStore: Send + Sync {
    fn get(&self, profile: &str) -> Result<Option<Secret>, KeyError>;
    fn set(&self, profile: &str, secret: &Secret) -> Result<(), KeyError>;
    /// Absent is `Ok`.
    fn delete(&self, profile: &str) -> Result<(), KeyError>;
    /// Profile ids that have a key.
    fn list(&self) -> Result<Vec<String>, KeyError>;
}

fn check_len(secret: &Secret) -> Result<(), KeyError> {
    if secret.expose().len() > MAX_KEY_BYTES {
        return Err(KeyError::TooLong);
    }
    Ok(())
}

/// Keys in memory (tests).
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, Secret>>);

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }
}

impl KeyStore for MemoryStore {
    fn get(&self, profile: &str) -> Result<Option<Secret>, KeyError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(profile)
            .cloned())
    }
    fn set(&self, profile: &str, secret: &Secret) -> Result<(), KeyError> {
        check_len(secret)?;
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(profile.to_string(), secret.clone());
        Ok(())
    }
    fn delete(&self, profile: &str) -> Result<(), KeyError> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(profile);
        Ok(())
    }
    fn list(&self) -> Result<Vec<String>, KeyError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect())
    }
}

/// Keys in a JSON file (`sonarad --keys fake`: `<home>\fake-keys.json`), a
/// testing aid that keeps conformance and e2e runs away from the real
/// Credential Manager. Never used for a user's keys.
pub struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    pub fn new(path: PathBuf) -> FileStore {
        FileStore {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn write(&self, m: &BTreeMap<String, String>) -> Result<(), KeyError> {
        let v = serde_json::Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        );
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, v.to_string())
            .and_then(|_| std::fs::rename(&tmp, &self.path))
            .map_err(|e| KeyError::Os(e.to_string()))
    }
}

impl KeyStore for FileStore {
    fn get(&self, profile: &str) -> Result<Option<Secret>, KeyError> {
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.read().remove(profile).map(Secret::new))
    }
    fn set(&self, profile: &str, secret: &Secret) -> Result<(), KeyError> {
        check_len(secret)?;
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut m = self.read();
        m.insert(profile.to_string(), secret.expose().to_string());
        self.write(&m)
    }
    fn delete(&self, profile: &str) -> Result<(), KeyError> {
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut m = self.read();
        if m.remove(profile).is_some() {
            self.write(&m)?;
        }
        Ok(())
    }
    fn list(&self) -> Result<Vec<String>, KeyError> {
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.read().into_keys().collect())
    }
}

/// Windows Credential Manager: generic credentials `sonara:<id>`, user
/// `sonara`, persisted for this user on this PC (not roaming).
pub struct CredentialStore;

#[cfg(windows)]
mod credman {
    use super::{KeyError, Secret, TARGET_PREFIX};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    /// HRESULT_FROM_WIN32(ERROR_NOT_FOUND).
    const NOT_FOUND: i32 = 0x8007_0490_u32 as i32;

    fn os(e: windows::core::Error) -> KeyError {
        KeyError::Os(e.message().to_string())
    }

    fn target(profile: &str) -> HSTRING {
        HSTRING::from(format!("{TARGET_PREFIX}{profile}"))
    }

    pub fn get(profile: &str) -> Result<Option<Secret>, KeyError> {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: a valid target string and out pointer; freed below.
        match unsafe { CredReadW(&target(profile), CRED_TYPE_GENERIC, None, &mut cred) } {
            Ok(()) => {}
            Err(e) if e.code().0 == NOT_FOUND => return Ok(None),
            Err(e) => return Err(os(e)),
        }
        // SAFETY: CredReadW succeeded, so `cred` points to a credential
        // whose blob has `CredentialBlobSize` bytes.
        let secret = unsafe {
            let c = &*cred;
            let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let s = String::from_utf8_lossy(blob).into_owned();
            CredFree(cred as *const _);
            s
        };
        Ok(Some(Secret::new(secret)))
    }

    pub fn set(profile: &str, secret: &Secret) -> Result<(), KeyError> {
        let mut target: Vec<u16> = format!("{TARGET_PREFIX}{profile}")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut comment: Vec<u16> = "Sonara speech engine key"
            .encode_utf16()
            .chain([0])
            .collect();
        let mut user: Vec<u16> = "sonara".encode_utf16().chain([0]).collect();
        let mut blob = secret.expose().as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: every pointer is valid for the call.
        let r = unsafe { CredWriteW(&cred, 0) };
        blob.iter_mut().for_each(|b| *b = 0);
        r.map_err(os)
    }

    pub fn delete(profile: &str) -> Result<(), KeyError> {
        // SAFETY: a valid target string.
        match unsafe { CredDeleteW(&target(profile), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code().0 == NOT_FOUND => Ok(()),
            Err(e) => Err(os(e)),
        }
    }

    pub fn list() -> Result<Vec<String>, KeyError> {
        let filter = HSTRING::from(format!("{TARGET_PREFIX}*"));
        let mut count = 0u32;
        let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: valid out pointers; freed below.
        match unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut creds) } {
            Ok(()) => {}
            Err(e) if e.code().0 == NOT_FOUND => return Ok(Vec::new()),
            Err(e) => return Err(os(e)),
        }
        let mut out = Vec::new();
        // SAFETY: `creds` holds `count` credential pointers.
        unsafe {
            for i in 0..count as usize {
                let c = &**creds.add(i);
                if c.Type != CRED_TYPE_GENERIC {
                    continue;
                }
                if let Ok(name) = c.TargetName.to_string() {
                    if let Some(id) = name.strip_prefix(TARGET_PREFIX) {
                        out.push(id.to_string());
                    }
                }
            }
            CredFree(creds as *const _);
        }
        out.sort();
        Ok(out)
    }
}

impl KeyStore for CredentialStore {
    fn get(&self, profile: &str) -> Result<Option<Secret>, KeyError> {
        #[cfg(windows)]
        return credman::get(profile);
        #[cfg(not(windows))]
        {
            let _ = profile;
            Err(KeyError::Unavailable)
        }
    }
    fn set(&self, profile: &str, secret: &Secret) -> Result<(), KeyError> {
        check_len(secret)?;
        #[cfg(windows)]
        return credman::set(profile, secret);
        #[cfg(not(windows))]
        {
            let _ = profile;
            Err(KeyError::Unavailable)
        }
    }
    fn delete(&self, profile: &str) -> Result<(), KeyError> {
        #[cfg(windows)]
        return credman::delete(profile);
        #[cfg(not(windows))]
        {
            let _ = profile;
            Err(KeyError::Unavailable)
        }
    }
    fn list(&self) -> Result<Vec<String>, KeyError> {
        #[cfg(windows)]
        return credman::list();
        #[cfg(not(windows))]
        Err(KeyError::Unavailable)
    }
}

/// Finds the key of a profile at the moment it is needed.
#[derive(Clone)]
pub struct KeyResolver {
    store: Arc<dyn KeyStore>,
}

impl KeyResolver {
    pub fn new(store: Arc<dyn KeyStore>) -> KeyResolver {
        KeyResolver { store }
    }

    pub fn store(&self) -> &Arc<dyn KeyStore> {
        &self.store
    }

    /// The key now: none for `KeyRef::None`, the store's entry for
    /// `CredMan`, the process environment for `Env(NAME)`. Read on every
    /// request (cheap), so a key set with `engine_key` applies to the next
    /// chunk.
    pub fn resolve(&self, profile: &Profile) -> Result<Option<Secret>, ExtError> {
        match &profile.key_ref {
            KeyRef::None => Ok(None),
            KeyRef::Env(name) => Ok(std::env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| Secret::new(v.trim()))),
            KeyRef::CredMan => match self.store.get(&profile.id) {
                Ok(k) => Ok(k.filter(|k| !k.expose().is_empty())),
                Err(KeyError::Unavailable) => Ok(None),
                Err(e) => Err(ExtError::new(
                    Reason::NoKey,
                    format!("cannot read the key of '{}': {e}", profile.id),
                )),
            },
        }
    }

    /// Whether a key resolves now (`engine_list.key_present`).
    pub fn present(&self, profile: &Profile) -> bool {
        matches!(self.resolve(profile), Ok(Some(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(key_ref: &str) -> Profile {
        Profile::from_json(&json!({"id": "p", "kind": "openai-compatible",
            "url": "https://tts.example.com/v1", "key_ref": key_ref}))
        .unwrap()
    }

    #[test]
    fn secrets_never_print() {
        let s = Secret::new("sk-very-secret-value-1234567890");
        assert_eq!(format!("{s:?}"), "[redacted]");
        assert_eq!(format!("{s}"), "[redacted]");
        assert_eq!(s.expose(), "sk-very-secret-value-1234567890");
    }

    #[test]
    fn memory_and_file_stores_round_trip() {
        let dir = std::env::temp_dir().join(format!("sonara-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = FileStore::new(dir.join("fake-keys.json"));
        let mem = MemoryStore::new();
        for store in [&file as &dyn KeyStore, &mem] {
            assert!(store.get("a").unwrap().is_none());
            store.set("a", &Secret::new("k1")).unwrap();
            store.set("b", &Secret::new("k2")).unwrap();
            assert_eq!(store.get("a").unwrap().unwrap().expose(), "k1");
            assert_eq!(store.list().unwrap(), vec!["a", "b"]);
            store.delete("a").unwrap();
            store.delete("a").unwrap();
            assert_eq!(store.list().unwrap(), vec!["b"]);
            assert_eq!(
                store.set("c", &Secret::new("x".repeat(MAX_KEY_BYTES + 1))),
                Err(KeyError::TooLong)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_resolver_reads_credman_env_or_nothing() {
        let store = Arc::new(MemoryStore::new());
        let keys = KeyResolver::new(store.clone());
        assert!(keys.resolve(&profile("none")).unwrap().is_none());
        assert!(!keys.present(&profile("credman")));
        store.set("p", &Secret::new("sk-abc")).unwrap();
        assert_eq!(
            keys.resolve(&profile("credman")).unwrap().unwrap().expose(),
            "sk-abc"
        );
        let var = format!("SONARA_TEST_KEY_{}", std::process::id());
        assert!(keys
            .resolve(&profile(&format!("env:{var}")))
            .unwrap()
            .is_none());
        std::env::set_var(&var, " from-env ");
        assert_eq!(
            keys.resolve(&profile(&format!("env:{var}")))
                .unwrap()
                .unwrap()
                .expose(),
            "from-env"
        );
        std::env::remove_var(&var);
    }
}
