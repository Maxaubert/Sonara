//! R7: sonara-hook is L5, an adapter that speaks protocol v1 to `sonarad`.
//! It links no runtime crate (the reading logic stays in the runtime), so
//! a dependency on any workspace crate fails here. Reads `cargo metadata`.
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
    let me = packages
        .iter()
        .find(|p| p["name"] == "sonara-hook")
        .expect("sonara-hook is a workspace member");
    let internal: Vec<&str> = me["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .filter(|n| workspace.contains(n))
        .collect();
    assert!(internal.is_empty(), "L5 links {internal:?}");
}
