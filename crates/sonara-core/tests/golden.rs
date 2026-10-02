use serde_json::Value;
use sonara_core::assembler::{Chunk, ProseAssembler};
use sonara_core::text::{clean_markdown, normalize_for_speech};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/text_rules")
}

fn assemble(deltas: &[String]) -> Vec<Value> {
    let mut a = ProseAssembler::new();
    let mut out = Vec::new();
    let last = deltas.len().saturating_sub(1);
    for (i, d) in deltas.iter().enumerate() {
        for c in a.feed(d, i as u32, i == last) {
            out.push(match c {
                Chunk::Text(s) => Value::String(s),
                Chunk::ParagraphBreak => Value::Null,
            });
        }
    }
    out
}

#[test]
fn every_golden_case_matches() {
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir(fixtures_dir()).expect("fixtures dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for case in data["cases"].as_array().unwrap() {
            count += 1;
            let name = format!(
                "{}::{}",
                path.file_stem().unwrap().to_string_lossy(),
                case["name"]
            );
            let ok = match case["fn"].as_str().unwrap() {
                "assemble" => {
                    let deltas: Vec<String> =
                        serde_json::from_value(case["deltas"].clone()).unwrap();
                    Value::Array(assemble(&deltas)) == case["output"]
                }
                "clean_markdown" => {
                    clean_markdown(case["input"].as_str().unwrap())
                        == case["output"].as_str().unwrap()
                }
                "normalize_for_speech" => {
                    normalize_for_speech(case["input"].as_str().unwrap())
                        == case["output"].as_str().unwrap()
                }
                other => panic!("unknown fn {other}"),
            };
            if !ok {
                failures.push(name);
            }
        }
    }
    assert!(count > 0, "no golden cases found");
    assert!(failures.is_empty(), "failing cases: {failures:#?}");
}
