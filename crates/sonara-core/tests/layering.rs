//! R7 layering guard: no L1 crate may depend, directly or through another
//! workspace crate, on an L2+ crate or a host. Reads `cargo metadata`, so it
//! covers every workspace member, including crates added later.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

const L1: [&str; 4] = [
    "sonara-core",
    "sonara-engine",
    "sonara-audio",
    "sonara-reader",
];
const ABOVE_L1: [&str; 5] = [
    "sonara-channels",
    "sonara-agent",
    "sonara-system",
    "sonarad",
    "sonara-hook",
];

/// Every (L1 crate, forbidden crate) pair reachable through declared
/// dependencies of workspace members (all kinds, dev and build included).
fn violations(metadata: &Value) -> Vec<(String, String)> {
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pkg in metadata["packages"].as_array().expect("packages") {
        let name = pkg["name"].as_str().expect("name").to_string();
        let list = pkg["dependencies"]
            .as_array()
            .expect("dependencies")
            .iter()
            .map(|d| d["name"].as_str().expect("dep name").to_string())
            .collect();
        deps.insert(name, list);
    }
    let mut out = Vec::new();
    for root in L1.iter().filter(|n| deps.contains_key(**n)) {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root.to_string()];
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            if ABOVE_L1.contains(&n.as_str()) {
                out.push((root.to_string(), n.clone()));
            }
            if let Some(next) = deps.get(&n) {
                stack.extend(next.iter().cloned());
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
fn l1_crates_do_not_depend_on_higher_layers() {
    let meta = workspace_metadata();
    let names: Vec<&str> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"sonara-core"), "{:?}", names);
    assert_eq!(violations(&meta), Vec::<(String, String)>::new());
}

#[test]
fn the_guard_catches_direct_and_transitive_violations() {
    let fake = serde_json::json!({ "packages": [
        { "name": "sonara-core", "dependencies": [{ "name": "regex" }] },
        { "name": "sonara-engine", "dependencies": [{ "name": "sonara-channels" }] },
        { "name": "sonara-reader", "dependencies": [{ "name": "helper" }] },
        { "name": "helper", "dependencies": [{ "name": "sonarad" }] },
        { "name": "sonara-channels", "dependencies": [{ "name": "sonara-core" }] },
        { "name": "sonarad", "dependencies": [] }
    ]});
    assert_eq!(
        violations(&fake),
        vec![
            ("sonara-engine".to_string(), "sonara-channels".to_string()),
            ("sonara-reader".to_string(), "sonarad".to_string()),
        ]
    );
}
