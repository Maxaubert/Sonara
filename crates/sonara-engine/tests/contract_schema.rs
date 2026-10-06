//! The contract helpers themselves (#275): the schema subset validator and
//! the path templates, so a contract test that passes is not a validator
//! that sees nothing.
mod common;

use common::contract::{match_template, Fragment, Mode};

#[test]
fn the_validator_catches_what_it_should() {
    let f = Fragment {
        name: "self".into(),
        root: serde_json::json!({"$defs": {
            "R": {"type": "object", "properties": {
                "a": {"type": "string", "enum": ["x", "y"]},
                "n": {"type": "number", "minimum": 0.5, "maximum": 2},
                "o": {"anyOf": [{"type": "string"}, {"$ref": "#/$defs/O"}]}
            }, "required": ["a"]},
            "O": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"],
                "additionalProperties": false}
        }}),
    };
    let r = f.def("R");
    let ok = serde_json::json!({"a": "x", "n": 1.25, "o": {"id": "v"}});
    assert!(f.errors(&r, &ok, Mode::Request).is_empty());
    for bad in [
        serde_json::json!({"n": 1}),
        serde_json::json!({"a": "z"}),
        serde_json::json!({"a": "x", "n": 2.5}),
        serde_json::json!({"a": "x", "o": {"name": "v"}}),
        serde_json::json!({"a": "x", "typo": 1}),
    ] {
        assert!(!f.errors(&r, &bad, Mode::Request).is_empty(), "{bad}");
    }
    assert!(f
        .errors(
            &r,
            &serde_json::json!({"a": "x", "extra": 1}),
            Mode::Response
        )
        .is_empty());
    assert_eq!(
        match_template(
            "/v1/text-to-speech/{voice_id}/stream",
            "/v1/text-to-speech/a%20b/stream"
        ),
        Some(vec![("voice_id".into(), "a b".into())])
    );
    assert_eq!(
        match_template(
            "/v1beta/models/{modelsId}:generateContent",
            "/v1beta/models/m-1:generateContent"
        ),
        Some(vec![("modelsId".into(), "m-1".into())])
    );
    assert_eq!(match_template("/v1/speak", "/v1/speak/x"), None);
}

/// Every `$ref` of a fragment points at one of its `$defs`.
fn refs(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(o) => {
            if let Some(r) = o.get("$ref").and_then(|r| r.as_str()) {
                out.push(r.to_string());
            }
            o.values().for_each(|x| refs(x, out));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

#[test]
fn every_fragment_is_whole_and_has_its_sources() {
    let root = common::contract::dir("");
    let mut seen = 0;
    for entry in std::fs::read_dir(&root).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let sources = std::fs::read_to_string(dir.join("SOURCES.md"))
            .unwrap_or_else(|_| panic!("{name}: SOURCES.md"));
        assert!(
            sources.contains("http"),
            "{name}: SOURCES.md names its URLs"
        );
        assert!(!sources.contains('\u{2014}'), "{name}: no em-dashes");
        let f = Fragment::load(&name);
        let mut all = Vec::new();
        refs(&f.root, &mut all);
        for r in all {
            let def = r
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("{name}: {r} is not a $defs ref"));
            assert!(f.root["$defs"].get(def).is_some(), "{name}: dangling {r}");
        }
        seen += 1;
    }
    assert!(seen >= 13, "{seen} providers");
}
