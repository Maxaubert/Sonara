//! R7: sonara-hook is L5, an adapter that speaks protocol v1 to `sonarad`.
//! It links no runtime crate (the reading logic stays in the runtime), so
//! a dependency on any workspace crate fails here, but `sonara-log` (the
//! shared log folder, #219), a leaf that itself depends on nothing. Reads
//! `cargo metadata`.
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn sonara_hook_depends_on_no_workspace_crate() {
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
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();
    let packages = meta["packages"].as_array().unwrap();
    let workspace: Vec<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let internal = |name: &str| -> Vec<String> {
        let me = packages
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("{name} is a workspace member"));
        me["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .filter(|n| workspace.contains(&n.as_str()))
            .collect()
    };
    let hook = internal("sonara-hook");
    assert!(hook.iter().all(|n| n == "sonara-log"), "L5 links {hook:?}");
    let log = internal("sonara-log");
    assert!(log.is_empty(), "sonara-log links {log:?}");
    let all: Vec<String> = packages.iter().find(|p| p["name"] == "sonara-log").unwrap()
        ["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_string())
        .collect();
    assert!(all.is_empty(), "sonara-log is a leaf: {all:?}");
}
