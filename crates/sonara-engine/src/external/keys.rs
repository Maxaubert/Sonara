//! Keys of external engines (spec section 6): Windows Credential Manager
//! (target `sonara:<profile id>`) or an environment variable the profile
//! names. A key is never in a file of the home (the `FileStore` is a
//! testing aid for `sonarad --keys fake`), never logged, never in a reply.
//!
//! A stored key is bound to the origin (`scheme://host:port`) it was
//! entered for (spec 6.4), kept with the secret (a credential attribute).
//! The resolver refuses a key whose origin is not the profile's now, so a
//! protocol client that retargets a profile never gets its key sent to
//! another server. An `env:` key goes only to the provider's default origin
//! or the one confirmed in `engines.json` (`Profile::key_origin`).
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
/// The credential attribute that holds the origin a key is bound to.
pub const ORIGIN_ATTRIBUTE: &str = "sonara-origin";
/// The longest origin a credential attribute holds (CRED_MAX_VALUE_SIZE).
pub const MAX_ORIGIN_BYTES: usize = 256;

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

/// A stored key and the origin it was entered for (`None`: stored before
/// keys were bound, or by an older runtime; never sent).
#[derive(Clone, Debug)]
pub struct StoredKey {
    pub secret: Secret,
    pub origin: Option<String>,
}

impl StoredKey {
    pub fn expose(&self) -> &str {
        self.secret.expose()
    }
}

/// Why a key store failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// No Credential Manager in this build (not Windows).
    Unavailable,
    TooLong,
    /// The origin does not fit a credential attribute.
    OriginTooLong,
    Os(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Unavailable => f.write_str("Credential Manager is not available"),
            KeyError::TooLong => write!(f, "key too long (at most {MAX_KEY_BYTES} bytes)"),
            KeyError::OriginTooLong => {
                write!(f, "address too long (at most {MAX_ORIGIN_BYTES} bytes)")
            }
            KeyError::Os(m) => write!(f, "Credential Manager: {m}"),
        }
    }
}

/// Where keys are kept, by profile id, each with the origin it is for.
pub trait KeyStore: Send + Sync {
    fn get(&self, profile: &str) -> Result<Option<StoredKey>, KeyError>;
    /// Store `secret` for `profile`, bound to `origin`.
    fn set(&self, profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError>;
    /// Absent is `Ok`.
    fn delete(&self, profile: &str) -> Result<(), KeyError>;
    /// Profile ids that have a key.
    fn list(&self) -> Result<Vec<String>, KeyError>;
}

fn check_len(secret: &Secret, origin: &str) -> Result<(), KeyError> {
    if secret.expose().len() > MAX_KEY_BYTES {
        return Err(KeyError::TooLong);
    }
    if origin.len() > MAX_ORIGIN_BYTES {
        return Err(KeyError::OriginTooLong);
    }
    Ok(())
}

/// Keys in memory (tests).
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, StoredKey>>);

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// A key with no origin, as stored before keys were bound (tests).
    pub fn set_unbound(&self, profile: &str, secret: &Secret) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(
            profile.to_string(),
            StoredKey {
                secret: secret.clone(),
                origin: None,
            },
        );
    }
}

impl KeyStore for MemoryStore {
    fn get(&self, profile: &str) -> Result<Option<StoredKey>, KeyError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(profile)
            .cloned())
    }
    fn set(&self, profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError> {
        check_len(secret, origin)?;
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(
            profile.to_string(),
            StoredKey {
                secret: secret.clone(),
                origin: Some(origin.to_string()),
            },
        );
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

    /// `{id: {"key": .., "origin": ..}}`; a bare string is a key with no
    /// origin (the format before keys were bound).
    fn read(&self) -> BTreeMap<String, (String, Option<String>)> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| match &v {
                        serde_json::Value::String(s) => Some((k, (s.clone(), None))),
                        serde_json::Value::Object(o) => {
                            let key = o.get("key")?.as_str()?.to_string();
                            let origin =
                                o.get("origin").and_then(|x| x.as_str()).map(str::to_string);
                            Some((k, (key, origin)))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn write(&self, m: &BTreeMap<String, (String, Option<String>)>) -> Result<(), KeyError> {
        let v = serde_json::Value::Object(
            m.iter()
                .map(|(k, (key, origin))| {
                    (k.clone(), serde_json::json!({"key": key, "origin": origin}))
                })
                .collect(),
        );
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, v.to_string())
            .and_then(|_| std::fs::rename(&tmp, &self.path))
            .map_err(|e| KeyError::Os(e.to_string()))
    }
}

impl KeyStore for FileStore {
    fn get(&self, profile: &str) -> Result<Option<StoredKey>, KeyError> {
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        Ok(self.read().remove(profile).map(|(key, origin)| StoredKey {
            secret: Secret::new(key),
            origin,
        }))
    }
    fn set(&self, profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError> {
        check_len(secret, origin)?;
        let _l = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut m = self.read();
        m.insert(
            profile.to_string(),
            (secret.expose().to_string(), Some(origin.to_string())),
        );
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
    use super::{KeyError, Secret, StoredKey, ORIGIN_ATTRIBUTE, TARGET_PREFIX};
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CREDENTIAL_ATTRIBUTEW, CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    /// HRESULT_FROM_WIN32(ERROR_NOT_FOUND).
    const NOT_FOUND: i32 = 0x8007_0490_u32 as i32;

    fn os(e: windows::core::Error) -> KeyError {
        KeyError::Os(e.message().to_string())
    }

    fn target(profile: &str) -> HSTRING {
        HSTRING::from(format!("{TARGET_PREFIX}{profile}"))
    }

    pub fn get(profile: &str) -> Result<Option<StoredKey>, KeyError> {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: a valid target string and out pointer; freed below.
        match unsafe { CredReadW(&target(profile), CRED_TYPE_GENERIC, None, &mut cred) } {
            Ok(()) => {}
            Err(e) if e.code().0 == NOT_FOUND => return Ok(None),
            Err(e) => return Err(os(e)),
        }
        // SAFETY: CredReadW succeeded, so `cred` points to a credential
        // whose blob has `CredentialBlobSize` bytes and whose `Attributes`
        // holds `AttributeCount` attributes.
        let (secret, origin) = unsafe {
            let c = &*cred;
            let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let s = String::from_utf8_lossy(blob).into_owned();
            let mut origin = None;
            if !c.Attributes.is_null() {
                let attrs = std::slice::from_raw_parts(c.Attributes, c.AttributeCount as usize);
                for a in attrs {
                    if a.Keyword.to_string().ok().as_deref() == Some(ORIGIN_ATTRIBUTE) {
                        let v = std::slice::from_raw_parts(a.Value, a.ValueSize as usize);
                        origin = std::str::from_utf8(v).ok().map(str::to_string);
                    }
                }
            }
            CredFree(cred as *const _);
            (s, origin)
        };
        Ok(Some(StoredKey {
            secret: Secret::new(secret),
            origin,
        }))
    }

    pub fn set(profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError> {
        let mut target: Vec<u16> = format!("{TARGET_PREFIX}{profile}")
            .encode_utf16()
            .chain([0])
            .collect();
        // Shown in Credential Manager; the attribute is what counts.
        let note = format!("Sonara speech engine key, sent only to {origin}");
        let note = if note.chars().count() < 256 {
            note
        } else {
            "Sonara speech engine key".to_string()
        };
        let mut comment: Vec<u16> = note.encode_utf16().chain([0]).collect();
        let mut user: Vec<u16> = "sonara".encode_utf16().chain([0]).collect();
        let mut keyword: Vec<u16> = ORIGIN_ATTRIBUTE.encode_utf16().chain([0]).collect();
        let mut value = origin.as_bytes().to_vec();
        let mut attribute = CREDENTIAL_ATTRIBUTEW {
            Keyword: PWSTR(keyword.as_mut_ptr()),
            Flags: 0,
            ValueSize: value.len() as u32,
            Value: value.as_mut_ptr(),
        };
        let mut blob = secret.expose().as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 1,
            Attributes: &mut attribute,
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
    fn get(&self, profile: &str) -> Result<Option<StoredKey>, KeyError> {
        #[cfg(windows)]
        return credman::get(profile);
        #[cfg(not(windows))]
        {
            let _ = profile;
            Err(KeyError::Unavailable)
        }
    }
    fn set(&self, profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError> {
        check_len(secret, origin)?;
        #[cfg(windows)]
        return credman::set(profile, secret, origin);
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
    ///
    /// A key is only returned for the origin it is bound to (spec 6.4): a
    /// stored key whose origin is not the profile's (or has none), or an
    /// env key for an origin that is neither the provider's default nor
    /// the confirmed `key_origin`, is a `no_key` error and never sent.
    pub fn resolve(&self, profile: &Profile) -> Result<Option<Secret>, ExtError> {
        let now = profile.origin();
        let address = now.clone().unwrap_or_else(|| "no address".into());
        match &profile.key_ref {
            KeyRef::None => Ok(None),
            KeyRef::Env(name) => {
                let Some(v) = std::env::var(name).ok().filter(|v| !v.trim().is_empty()) else {
                    return Ok(None);
                };
                let allowed =
                    now.is_some() && (now == profile.default_origin() || now == profile.key_origin);
                if !allowed {
                    return Err(ExtError::new(
                        Reason::NoKey,
                        format!(
                            "the key in {name} is not sent to {address}: an address set over \
                             the protocol must be confirmed in engines.json (\"key_origin\": \
                             \"{address}\" in engine '{}'), or use a stored key",
                            profile.id
                        ),
                    ));
                }
                Ok(Some(Secret::new(v.trim())))
            }
            KeyRef::CredMan => match self.store.get(&profile.id) {
                Ok(None) => Ok(None),
                Ok(Some(k)) if k.expose().is_empty() => Ok(None),
                Ok(Some(k)) => match &k.origin {
                    Some(o) if Some(o) == now.as_ref() => Ok(Some(k.secret)),
                    Some(o) => Err(ExtError::new(
                        Reason::NoKey,
                        format!(
                            "the key of '{}' was entered for {o}, not {address}: enter the \
                             key again",
                            profile.id
                        ),
                    )),
                    None => Err(ExtError::new(
                        Reason::NoKey,
                        format!(
                            "the key of '{}' is not bound to an address: enter the key again",
                            profile.id
                        ),
                    )),
                },
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
            store
                .set("a", &Secret::new("k1"), "https://a.example:443")
                .unwrap();
            store
                .set("b", &Secret::new("k2"), "https://b.example:443")
                .unwrap();
            assert_eq!(store.get("a").unwrap().unwrap().expose(), "k1");
            assert_eq!(
                store.get("a").unwrap().unwrap().origin.as_deref(),
                Some("https://a.example:443")
            );
            assert_eq!(store.list().unwrap(), vec!["a", "b"]);
            store.delete("a").unwrap();
            store.delete("a").unwrap();
            assert_eq!(store.list().unwrap(), vec!["b"]);
            assert_eq!(
                store.set(
                    "c",
                    &Secret::new("x".repeat(MAX_KEY_BYTES + 1)),
                    "https://c:443"
                ),
                Err(KeyError::TooLong)
            );
            assert_eq!(
                store.set("c", &Secret::new("k"), &"o".repeat(MAX_ORIGIN_BYTES + 1)),
                Err(KeyError::OriginTooLong)
            );
        }
        // The file store reads its format before keys were bound: no origin.
        std::fs::write(dir.join("fake-keys.json"), r#"{"old": "k0"}"#).unwrap();
        let old = file.get("old").unwrap().unwrap();
        assert_eq!((old.expose(), old.origin.as_deref()), ("k0", None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_resolver_reads_credman_env_or_nothing() {
        let store = Arc::new(MemoryStore::new());
        let keys = KeyResolver::new(store.clone());
        assert!(keys.resolve(&profile("none")).unwrap().is_none());
        assert!(!keys.present(&profile("credman")));
        store
            .set("p", &Secret::new("sk-abc"), "https://tts.example.com:443")
            .unwrap();
        assert_eq!(
            keys.resolve(&profile("credman")).unwrap().unwrap().expose(),
            "sk-abc"
        );
        let var = format!("SONARA_TEST_KEY_{}", std::process::id());
        let mut env = profile(&format!("env:{var}"));
        env.key_origin = Some("https://tts.example.com:443".into());
        assert!(keys.resolve(&env).unwrap().is_none());
        std::env::set_var(&var, " from-env ");
        assert_eq!(keys.resolve(&env).unwrap().unwrap().expose(), "from-env");
        std::env::remove_var(&var);
    }

    #[test]
    fn a_key_is_only_resolved_for_its_origin() {
        let store = Arc::new(MemoryStore::new());
        let keys = KeyResolver::new(store.clone());
        store
            .set("p", &Secret::new("sk-abc"), "https://other.example:443")
            .unwrap();
        let e = keys.resolve(&profile("credman")).unwrap_err();
        assert_eq!(e.reason, Reason::NoKey);
        assert_eq!(
            e.message,
            "the key of 'p' was entered for https://other.example:443, not \
             https://tts.example.com:443: enter the key again"
        );
        assert!(!keys.present(&profile("credman")));
        // Another port is another origin.
        store
            .set("p", &Secret::new("sk-abc"), "https://tts.example.com:8443")
            .unwrap();
        assert!(keys.resolve(&profile("credman")).is_err());
        // No origin recorded: refused, never bound on use.
        store.set_unbound("p", &Secret::new("sk-abc"));
        assert!(keys.resolve(&profile("credman")).is_err());
        assert!(keys.resolve(&profile("credman")).is_err(), "still refused");
    }

    #[test]
    fn an_env_key_goes_to_the_provider_or_the_confirmed_origin_only() {
        let store = Arc::new(MemoryStore::new());
        let keys = KeyResolver::new(store);
        let var = format!("SONARA_TEST_ENV_ORIGIN_{}_API_KEY", std::process::id());
        std::env::set_var(&var, "sk-env");
        let p = |v: serde_json::Value| Profile::from_json(&v).unwrap();
        // The provider's own address: always.
        let openai = p(json!({"id": "o", "kind": "openai-compatible",
            "options": {"preset": "openai"}, "key_ref": format!("env:{var}")}));
        assert_eq!(keys.resolve(&openai).unwrap().unwrap().expose(), "sk-env");
        // The same preset pointed elsewhere: only when confirmed.
        let mut proxied = p(json!({"id": "o", "kind": "openai-compatible",
            "url": "https://proxy.example.com/v1",
            "options": {"preset": "openai"}, "key_ref": format!("env:{var}")}));
        let e = keys.resolve(&proxied).unwrap_err();
        assert_eq!(e.reason, Reason::NoKey);
        assert!(e.message.contains("key_origin"), "{}", e.message);
        assert!(!e.message.contains("sk-env"));
        proxied.key_origin = Some("https://other.example:443".into());
        assert!(keys.resolve(&proxied).is_err());
        proxied.key_origin = Some("https://proxy.example.com:443".into());
        assert_eq!(keys.resolve(&proxied).unwrap().unwrap().expose(), "sk-env");
        std::env::remove_var(&var);
    }
}
