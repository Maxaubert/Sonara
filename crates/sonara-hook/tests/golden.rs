//! Golden cases (`tests/golden/*.json`): each Claude Code hook event (the
//! payloads captured in the repo's `tests/fixtures/` and a few inline ones)
//! must map to exactly the messages listed. The cases are the contract of
//! the hook mapping: change one only with a deliberate change to the mapping.
use serde_json::{Map, Value};
use sonara_hook::map_event;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn every_golden_case_maps_to_its_messages() {
    let mut n = 0;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir().join("tests/golden"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let case: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let payload = match case.get("fixture").and_then(Value::as_str) {
            Some(f) => {
                let p = dir().join("../../tests/fixtures").join(f);
                serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
            }
            None => case["payload"].clone(),
        };
        let env: Map<String, Value> = case["env"].as_object().cloned().unwrap_or_default();
        let lookup = |k: &str| env.get(k).and_then(Value::as_str).map(str::to_string);
        let got = map_event(case["event"].as_str().unwrap(), &payload, &lookup);
        assert_eq!(
            Value::Array(got),
            case["messages"],
            "{}",
            path.file_name().unwrap().to_string_lossy()
        );
        n += 1;
    }
    assert!(n >= 20, "only {n} golden cases");
}

#[test]
fn every_captured_payload_has_a_golden_case() {
    let used: std::collections::BTreeSet<String> = std::fs::read_dir(dir().join("tests/golden"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| {
            let case: Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
            case.get("fixture")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let captured: Vec<String> = std::fs::read_dir(dir().join("../../tests/fixtures"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        !captured.is_empty(),
        "no captured payloads in tests/fixtures"
    );
    let missing: Vec<&String> = captured.iter().filter(|f| !used.contains(*f)).collect();
    assert!(
        missing.is_empty(),
        "captured payloads with no golden case: {missing:?}"
    );
}
