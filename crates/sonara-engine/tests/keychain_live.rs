//! A real macOS Keychain round trip (opt-in): `cargo test -p sonara-engine
//! --test keychain_live -- --ignored`. It writes, reads, lists and deletes
//! the generic passwords `sonara` / `test-<pid>` and `sonara-test` /
//! `test-<pid>` in the login keychain only. User interaction is turned off
//! first, so nothing is ever shown: a locked keychain fails the test
//! instead of asking.
#![cfg(target_os = "macos")]
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::passwords::{delete_generic_password, set_generic_password};
use sonara_engine::external::keys::{KeyStore, KeychainStore, Secret, KEYCHAIN_SERVICE};

#[test]
#[ignore]
fn keychain_round_trip() {
    let _quiet = SecKeychain::disable_user_interaction().unwrap();
    let id = format!("test-{}", std::process::id());
    let store = KeychainStore;
    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
    store
        .set(
            &id,
            &Secret::new("sk-live-keychain-check"),
            "https://api.example.com:443",
        )
        .unwrap();
    let got = store.get(&id).unwrap().unwrap();
    assert_eq!(got.expose(), "sk-live-keychain-check");
    assert_eq!(
        got.origin.as_deref(),
        Some("https://api.example.com:443"),
        "the origin is kept with the secret"
    );
    // A second set replaces the item (one item per profile).
    store
        .set(&id, &Secret::new("sk-two"), "https://b.example.com:443")
        .unwrap();
    let got = store.get(&id).unwrap().unwrap();
    assert_eq!(
        (got.expose(), got.origin.as_deref()),
        ("sk-two", Some("https://b.example.com:443"))
    );
    assert_eq!(
        store.list().unwrap().iter().filter(|a| **a == id).count(),
        1
    );
    store.delete(&id).unwrap();
    store.delete(&id).unwrap();
    assert!(store.get(&id).unwrap().is_none());
    assert!(!store.list().unwrap().contains(&id));
}

/// `list` (what uninstall deletes) is the service `sonara` exactly: an
/// item of another service that starts with it is never listed.
#[test]
#[ignore]
fn keychain_list_matches_the_whole_service_name() {
    let _quiet = SecKeychain::disable_user_interaction().unwrap();
    let id = format!("test-{}", std::process::id());
    let other = format!("{KEYCHAIN_SERVICE}-test");
    set_generic_password(&other, &id, b"not sonara's").unwrap();
    let listed = KeychainStore.list();
    let _ = delete_generic_password(&other, &id);
    assert!(!listed.unwrap().contains(&id));
}
