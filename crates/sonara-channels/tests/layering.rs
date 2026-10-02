//! R7: sonara-channels is L2 and builds only on L1 crates. Reads
//! `cargo metadata`, so a new dependency on an L2+ crate or a host fails
//! here.
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

const L1: [&str; 4] = [
    "sonara-core",
    "sonara-engine",
    "sonara-audio",
    "sonara-reader",
];

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
fn sonara_channels_depends_only_on_l1_workspace_crates() {
    let meta = workspace_metadata();
    let packages = meta["packages"].as_array().unwrap();
    let workspace: Vec<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let me = packages
        .iter()
        .find(|p| p["name"] == "sonara-channels")
        .expect("sonara-channels is a workspace member");
    let mut internal: Vec<&str> = me["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .filter(|n| workspace.contains(n))
        .collect();
    internal.sort();
    internal.dedup();
    let above: Vec<&str> = internal
        .iter()
        .copied()
        .filter(|n| !L1.contains(n))
        .collect();
    assert!(above.is_empty(), "L2 depends on {above:?}");
    assert!(internal.contains(&"sonara-reader"), "{internal:?}");
}
