//! A real Windows Credential Manager round trip (opt-in): `cargo test -p
//! sonara-engine --test credman_live -- --ignored`. It writes, reads, lists
//! and deletes `sonara:test-<pid>` only.
use sonara_engine::external::keys::{CredentialStore, KeyStore, Secret};

#[test]
#[ignore]
fn credman_round_trip() {
    let id = format!("test-{}", std::process::id());
    let store = CredentialStore;
    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
    store
        .set(
            &id,
            &Secret::new("sk-live-credman-check"),
            "https://api.example.com:443",
        )
        .unwrap();
    let got = store.get(&id).unwrap().unwrap();
    assert_eq!(got.expose(), "sk-live-credman-check");
    assert_eq!(
        got.origin.as_deref(),
        Some("https://api.example.com:443"),
        "the origin is kept with the secret"
    );
    assert!(store.list().unwrap().contains(&id));
    store.delete(&id).unwrap();
    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
    assert!(!store.list().unwrap().contains(&id));
}
