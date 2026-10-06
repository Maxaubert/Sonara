//! R7 layering guard for the whole workspace: one table of the workspace
//! crates each crate may depend on (any kind: normal, dev and build). Reads
//! `cargo metadata`, so a new upward dependency fails here, and so does a
//! new workspace crate until it gets a row.
//!
//! Layers (CLAUDE.md, docs/architecture.md): L1 `sonara-core`, `-engine`,
//! `-audio`, `-reader` and the leaves `misaki` and `sonara-log`; L2
//! `sonara-channels`; L3 `sonara-agent`; L4 `sonara-system` (L1 only: the
//! host maps hotkey actions to L2 and L3); L5 `sonara-hook` (an adapter
//! that speaks protocol v1, so no runtime crate) and `sonara-cli`, both on
//! the leaf protocol client `sonara-client` (#255); `sonarad` hosts them
//! and shares the client's home and file names. Every allowed edge points down, so following
//! allowed edges never climbs either.
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

const L1: &[&str] = &[
    "misaki",
    "sonara-log",
    "sonara-core",
    "sonara-engine",
    "sonara-audio",
    "sonara-reader",
];
const L1_L2: &[&str] = &[
    "misaki",
    "sonara-log",
    "sonara-core",
    "sonara-engine",
    "sonara-audio",
    "sonara-reader",
    "sonara-channels",
];
const RUNTIME: &[&str] = &[
    "sonara-client",
    "misaki",
    "sonara-log",
    "sonara-core",
    "sonara-engine",
    "sonara-audio",
    "sonara-reader",
    "sonara-channels",
    "sonara-agent",
    "sonara-system",
];

/// Each workspace crate and the workspace crates it may depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    ("misaki", &[]),
    ("sonara-log", &[]),
    ("sonara-core", L1),
    ("sonara-engine", L1),
    ("sonara-audio", L1),
    ("sonara-reader", L1),
    ("sonara-channels", L1),
    ("sonara-agent", L1_L2),
    ("sonara-system", L1),
    ("sonara-client", &[]),
    ("sonara-hook", &["sonara-client", "sonara-log"]),
    ("sonara-cli", &["sonara-client", "sonara-log"]),
    ("sonarad", RUNTIME),
];

/// The only outside crate the leaf `sonara-log` may use (for `scrub`), so
/// the hook links no more than it already does.
const LOG_EXTERNAL: &[&str] = &["serde_json"];

/// Every package's dependency names, from `cargo metadata --no-deps`.
fn dependencies(metadata: &Value) -> BTreeMap<String, Vec<String>> {
    metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .map(|d| d["name"].as_str().expect("dep name").to_string())
                .collect();
            (p["name"].as_str().expect("name").to_string(), deps)
        })
        .collect()
}

/// Every (crate, workspace dependency) edge the table does not allow, and
/// every workspace crate without a row (as `(crate, "no row")`).
fn violations(metadata: &Value) -> Vec<(String, String)> {
    let deps = dependencies(metadata);
    let mut out = Vec::new();
    for (name, list) in &deps {
        let Some((_, allowed)) = ALLOWED.iter().find(|(n, _)| n == name) else {
            out.push((name.clone(), "no row".to_string()));
            continue;
        };
        for d in list {
            let internal = deps.contains_key(d) && d != name;
            if internal
                && !allowed.contains(&d.as_str())
                && !out.contains(&(name.clone(), d.clone()))
            {
                out.push((name.clone(), d.clone()));
            }
        }
    }
    out
}

fn workspace_metadata() -> Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("metadata json")
}

#[test]
fn every_workspace_crate_depends_only_on_its_allowed_layers() {
    let meta = workspace_metadata();
    let deps = dependencies(&meta);
    for (name, _) in ALLOWED {
        assert!(deps.contains_key(*name), "{name} is a workspace member");
    }
    assert_eq!(violations(&meta), Vec::<(String, String)>::new());
}

#[test]
fn sonara_log_stays_a_leaf() {
    let deps = dependencies(&workspace_metadata());
    let log = &deps["sonara-log"];
    let extra: Vec<&String> = log
        .iter()
        .filter(|d| !LOG_EXTERNAL.contains(&d.as_str()))
        .collect();
    assert!(extra.is_empty(), "sonara-log links {extra:?}");
}

/// The protocol client links only `serde_json`, so the hook (a process
/// per Claude Code event) stays small.
const CLIENT_EXTERNAL: &[&str] = &["serde_json"];

#[test]
fn sonara_client_stays_a_leaf() {
    let deps = dependencies(&workspace_metadata());
    let client = &deps["sonara-client"];
    let extra: Vec<&String> = client
        .iter()
        .filter(|d| !CLIENT_EXTERNAL.contains(&d.as_str()))
        .collect();
    assert!(extra.is_empty(), "sonara-client links {extra:?}");
}

#[test]
fn the_guard_catches_upward_edges_and_unlisted_crates() {
    let fake = serde_json::json!({ "packages": [
        { "name": "sonara-core", "dependencies": [{ "name": "regex" }] },
        { "name": "sonara-engine", "dependencies": [{ "name": "sonara-channels" }] },
        { "name": "sonara-reader", "dependencies": [{ "name": "sonara-reader" }] },
        { "name": "sonara-channels", "dependencies": [{ "name": "sonara-core" }] },
        { "name": "sonara-system", "dependencies": [{ "name": "sonara-agent" }] },
        { "name": "sonara-agent", "dependencies": [{ "name": "sonara-channels" }] },
        { "name": "sonara-hook", "dependencies": [{ "name": "sonara-reader" }] },
        { "name": "helper", "dependencies": [] }
    ]});
    assert_eq!(
        violations(&fake),
        vec![
            ("helper".to_string(), "no row".to_string()),
            ("sonara-engine".to_string(), "sonara-channels".to_string()),
            ("sonara-hook".to_string(), "sonara-reader".to_string()),
            ("sonara-system".to_string(), "sonara-agent".to_string()),
        ]
    );
}
