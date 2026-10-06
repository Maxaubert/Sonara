//! A local server named by loopback (#274 live findings): `localhost` must
//! not pay Windows' 2 s refused connect on `::1` before trying 127.0.0.1,
//! and a stopped local server must fail fast so the fallback speaks
//! without a long silence.
mod common;

use common::{Route, ScriptServer};
use serde_json::json;
use sonara_engine::external::keys::{KeyResolver, MemoryStore};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::Engine;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn engine(url: &str) -> External {
    let profile = Profile::from_json(&json!({
        "id": "local", "kind": "openai-compatible", "url": url, "voice": "v1",
        "options": {"preset": "generic"},
    }))
    .unwrap();
    let keys = KeyResolver::new(Arc::new(MemoryStore::new()));
    External::new(ExternalConfig::new(profile, keys)).unwrap()
}

fn speak(e: &External) -> sonara_engine::Result<usize> {
    let chunks = e
        .synthesize("Hi.", "", 200)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(chunks.iter().map(|c| c.samples.len()).sum())
}

#[test]
fn a_localhost_url_reaches_an_ipv4_server_without_the_ipv6_delay() {
    let server = ScriptServer::start();
    server.on("/v1/audio/speech", Route::wav(&[7; 240], 24_000));
    let port = server.base.rsplit(':').next().unwrap();
    // A new engine each time: a new agent, so a new connection.
    for _ in 0..2 {
        let e = engine(&format!("http://localhost:{port}/v1"));
        let t = Instant::now();
        assert_eq!(speak(&e).unwrap(), 240);
        let took = t.elapsed();
        assert!(
            took < Duration::from_millis(1000),
            "localhost took {took:?}"
        );
    }
}

#[test]
fn a_stopped_local_server_fails_fast() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    for host in ["127.0.0.1", "localhost"] {
        let e = engine(&format!("http://{host}:{port}/v1"));
        let t = Instant::now();
        assert!(speak(&e).is_err(), "{host}: nothing listens");
        let took = t.elapsed();
        assert!(
            took < Duration::from_millis(1500),
            "{host}: failing took {took:?}"
        );
    }
}
