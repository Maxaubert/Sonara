//! A loopback engine never goes through the system proxy (review of #224):
//! a key bound to `http://127.0.0.1:<port>` would otherwise reach the proxy
//! in clear text. One test per binary: it sets the proxy environment.
mod common;

use common::{Route, ScriptServer};
use serde_json::json;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::Engine;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn a_loopback_engine_bypasses_the_system_proxy() {
    let provider = ScriptServer::start();
    provider.on("/v1/audio/speech", Route::wav(&[5], 24_000));
    let proxy = ScriptServer::start();
    std::env::set_var("ALL_PROXY", &proxy.base);
    std::env::set_var("NO_PROXY", "nothing.invalid");

    // The control: an agent with the proxy in force sends there.
    let plain = sonara_engine::http::agent(sonara_engine::http::Timeouts {
        connect: Duration::from_secs(5),
        recv_response: Duration::from_secs(5),
        recv_body: Duration::from_secs(5),
    });
    let _ = plain.get(format!("{}/v1/models", provider.base)).call();
    assert_eq!(proxy.requests().len(), 1, "the proxy environment applies");

    let profile = Profile::from_json(&json!({"id": "local", "kind": "openai-compatible",
        "url": format!("{}/v1", provider.base), "key_ref": "credman",
        "options": {"preset": "generic"}}))
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    store
        .set("local", &Secret::new("sk-loop"), &profile.origin().unwrap())
        .unwrap();
    let e = External::new(ExternalConfig::new(profile, KeyResolver::new(store))).unwrap();
    let out: Vec<i16> = e
        .synthesize("Hi.", "", 200)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    assert_eq!(out, vec![5]);
    assert_eq!(proxy.requests().len(), 1, "nothing more went to the proxy");
    let seen = provider.requests();
    assert_eq!(
        seen.last().unwrap().header("authorization"),
        Some("Bearer sk-loop")
    );
}
