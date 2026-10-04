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
        .set(&id, &Secret::new("sk-live-credman-check"))
        .unwrap();
    assert_eq!(
        store.get(&id).unwrap().unwrap().expose(),
        "sk-live-credman-check"
    );
    assert!(store.list().unwrap().contains(&id));
    store.delete(&id).unwrap();
    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
    assert!(!store.list().unwrap().contains(&id));
}
