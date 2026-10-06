use super::*;
use sonara_engine::external::keys::{KeyStore, MemoryStore, Secret};
use sonara_engine::fake::FakeEngine;
use sonara_engine::LicenseClass;

fn home() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "sonarad-engines-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn setup(home: &Path, store: Arc<MemoryStore>) -> Setup {
    Setup {
        home: home.to_path_buf(),
        store,
        fallback: Some(Arc::new(FakeEngine::new())),
        fallback_voice: String::new(),
        default_engine: "fake".into(),
        log: None,
    }
}

fn registry() -> Arc<Registry> {
    Arc::new(Registry::new(&[
        LicenseClass::Permissive,
        LicenseClass::External,
    ]))
}

#[test]
fn engines_json_round_trip_keeps_unknown_kinds_and_never_a_key() {
    let h = home();
    std::fs::write(
        h.join(FILE),
        r#"{"format": 1, "engines": [
            {"id": "ca", "kind": "future-kind", "voice": "abc", "key_ref": "credman"},
            {"id": "bad", "kind": "openai-compatible", "url": "ftp://x"},
            {"id": "loc", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
             "options": {"preset": "kokoro-fastapi"}}]}"#,
    )
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    let (e, problems) = Engines::load(setup(&h, store.clone()));
    assert_eq!(problems.len(), 2, "{problems:?}");
    let reg = registry();
    e.attach(reg.clone());
    assert_eq!(
        reg.ids().iter().map(|i| i.as_str()).collect::<Vec<_>>(),
        vec!["loc"]
    );
    let list = e.list("loc");
    let views = list["engines"].as_array().unwrap();
    assert_eq!(views[0]["supported"], false);
    assert_eq!(views[1]["supported"], true);
    assert!(views[1]["error"].as_str().unwrap().contains("invalid url"));
    assert_eq!(views[2]["current"], true);
    assert!(views[2].get("model").is_none(), "no model filled in (#235)");
    assert_eq!(views[2]["missing"], json!(["voice"]));
    assert_eq!(views[2]["local"], true);
    assert_eq!(views[2]["status"]["status"], "ready");
    assert_eq!(
        list["kinds"],
        json!([
            "openai-compatible",
            "elevenlabs",
            "azure",
            "google",
            "gemini",
            "cartesia",
            "deepgram",
            "command"
        ])
    );
    // A new profile with a secret: the file keeps the others and no key.
    e.add(
        &json!({"id": "openai", "kind": "openai-compatible",
            "options": {"preset": "openai"}, "api_key": "sk-in-the-wrong-place-123"}),
        Some("sk-secret-value-1234567890"),
        false,
    )
    .unwrap();
    let text = std::fs::read_to_string(h.join(FILE)).unwrap();
    assert!(!text.contains("sk-"), "{text}");
    assert!(text.contains("future-kind") && text.contains("ftp://x"));
    assert_eq!(
        store.get("openai").unwrap().unwrap().expose(),
        "sk-secret-value-1234567890"
    );
    assert!(reg.get("openai").is_ok());
    let (again, _) = Engines::load(setup(&h, store));
    assert_eq!(again.ids(), vec!["ca", "bad", "loc", "openai"]);
    let _ = std::fs::remove_dir_all(&h);
}

/// `sonara engines add` writes engines.json as the user and then
/// reloads; a protocol add or remove that saves in between must not
/// write its older list over the user's new entry.
#[test]
fn a_save_never_loses_an_entry_written_to_the_file_meanwhile() {
    let h = home();
    let loc = |id: &str| {
        json!({"id": id, "kind": "openai-compatible",
            "url": "http://127.0.0.1:9/v1", "options": {"preset": "kokoro-fastapi"}})
    };
    let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    let reg = registry();
    e.attach(reg.clone());
    e.add(&loc("a"), None, false).unwrap();
    // The user's own edit (the CLI's write), not yet reloaded.
    let write_with = |ids: &[&str]| {
        let engines: Vec<Value> = ids.iter().map(|i| loc(i)).collect();
        std::fs::write(
            h.join(FILE),
            json!({"format": 1, "engines": engines}).to_string(),
        )
        .unwrap();
    };
    write_with(&["a", "mine"]);
    e.add(&loc("b"), None, false).unwrap();
    let (again, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    assert_eq!(again.ids(), vec!["a", "mine", "b"]);
    assert!(
        reg.get("mine").is_ok(),
        "the user's entry is registered too"
    );
    write_with(&["a", "mine", "b", "mine2"]);
    e.remove("a", false).unwrap();
    let (again, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    assert_eq!(again.ids(), vec!["mine", "b", "mine2"]);
    // A file the user broke is never overwritten by a protocol save.
    std::fs::write(h.join(FILE), "{ not json").unwrap();
    let err = e.add(&loc("c"), None, false).unwrap_err();
    assert!(err.message.contains("not valid JSON"), "{}", err.message);
    assert_eq!(std::fs::read_to_string(h.join(FILE)).unwrap(), "{ not json");
    let _ = std::fs::remove_dir_all(&h);
}

/// The view fills in the values in force, and `explicit` keeps what the
/// profile itself sets, so an edit form does not pin the defaults.
#[test]
fn the_view_tells_explicit_values_from_defaults() {
    let h = home();
    let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    e.add(
        &json!({"id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}),
        None,
        false,
    )
    .unwrap();
    e.add(
        &json!({"id": "az", "kind": "azure", "voice": "en-US-VoiceANeural",
            "options": {"region": "westeurope"}}),
        None,
        false,
    )
    .unwrap();
    e.add(
        &json!({"id": "own", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
            "model": "m1", "voice": "v1", "options": {"preset": "generic"}}),
        None,
        false,
    )
    .unwrap();
    let list = e.list("");
    let views = list["engines"].as_array().unwrap();
    assert_eq!(views[0]["url"], "https://api.openai.com/v1");
    assert_eq!(views[0]["explicit"], json!({}));
    // "Send to the engine" (#235): the cloud default in force, not
    // explicit; a local server's default is a sentence at a time.
    assert_eq!(views[0]["send_mode"], "message");
    assert_eq!(views[2]["send_mode"], "sentence");
    assert_eq!(
        views[1]["url"],
        "https://westeurope.tts.speech.microsoft.com"
    );
    assert_eq!(views[1]["explicit"], json!({"voice": "en-US-VoiceANeural"}));
    assert_eq!(
        views[2]["explicit"],
        json!({"url": "http://127.0.0.1:9/v1", "model": "m1", "voice": "v1"})
    );
    let _ = std::fs::remove_dir_all(&h);
}

/// "Send to the engine" (#235): a choice is stored and explicit, a
/// default is neither, and an edit back to the default forgets it.
#[test]
fn send_mode_is_stored_only_when_chosen() {
    let h = home();
    let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    e.add(
        &json!({"id": "el", "kind": "elevenlabs", "send_mode": "sentence"}),
        None,
        false,
    )
    .unwrap();
    let v = e.view_of("el", "").unwrap();
    assert_eq!(v["send_mode"], "sentence");
    assert_eq!(v["explicit"]["send_mode"], "sentence");
    let file = std::fs::read_to_string(e.file()).unwrap();
    assert!(file.contains("\"send_mode\": \"sentence\""), "{file}");
    e.add(&json!({"id": "el", "kind": "elevenlabs"}), None, true)
        .unwrap();
    let v = e.view_of("el", "").unwrap();
    assert_eq!(v["send_mode"], "message");
    assert!(v["explicit"].get("send_mode").is_none());
    let bad = e.add(
        &json!({"id": "x", "kind": "elevenlabs", "send_mode": "whole"}),
        None,
        false,
    );
    assert!(bad.is_err());
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn add_rules() {
    let h = home();
    let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    let local = json!({"id": "loc", "kind": "openai-compatible",
        "url": "http://127.0.0.1:9/v1", "options": {"preset": "kokoro-fastapi"}});
    assert!(!e.add(&local, None, false).unwrap());
    let err = e.add(&local, None, false).unwrap_err();
    assert_eq!(err.message, "engine 'loc' exists; send replace: true");
    assert!(e.add(&local, None, true).unwrap(), "a replace");
    let mut env = local.clone();
    env["id"] = json!("env1");
    env["key_ref"] = json!("env:MY_API_KEY");
    assert!(e
        .add(&env, Some("sk-x"), false)
        .unwrap_err()
        .message
        .contains("MY_API_KEY"));
    let mut none = local.clone();
    none["id"] = json!("n1");
    none["key_ref"] = json!("none");
    assert_eq!(
        e.add(&none, Some("k"), false).unwrap_err().code,
        Code::BadRequest
    );
    assert_eq!(
        e.add(
            &json!({"id": "kokoro", "kind": "openai-compatible"}),
            None,
            false
        )
        .unwrap_err()
        .message,
        "'kokoro' is a built-in engine"
    );
    assert_eq!(
        e.add(&json!({"id": "x", "kind": "future-kind"}), None, false)
            .unwrap_err()
            .code,
        Code::Unsupported
    );
    for i in 1..MAX_PROFILES {
        let mut p = local.clone();
        p["id"] = json!(format!("p{i}"));
        e.add(&p, None, false).unwrap();
    }
    let mut p = local.clone();
    p["id"] = json!("one-too-many");
    assert_eq!(
        e.add(&p, None, false).unwrap_err().message,
        "at most 16 engines"
    );
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn keys_and_removal() {
    let h = home();
    let store = Arc::new(MemoryStore::new());
    let (e, _) = Engines::load(setup(&h, store.clone()));
    let reg = registry();
    e.attach(reg.clone());
    e.add(
        &json!({"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1"}),
        None,
        false,
    )
    .unwrap();
    assert!(e.set_key("c", Some("sk-1")).unwrap());
    assert!(!e.set_key("c", None).unwrap());
    assert_eq!(e.set_key("nope", None).unwrap_err().code, Code::NotFound);
    e.set_key("c", Some("sk-2")).unwrap();
    e.remove("c", true).unwrap();
    assert!(store.get("c").unwrap().is_none(), "the key went with it");
    assert!(reg.get("c").is_err());
    assert_eq!(e.remove("c", true).unwrap_err().code, Code::NotFound);
    let _ = std::fs::remove_dir_all(&h);
}

/// A stored profile whose program went away stays registered (it reads
/// with the fallback until the program is back). A command profile is
/// never added through `add` (the protocol): only from the file.
#[test]
fn command_profiles_come_only_from_the_file() {
    let h = home();
    let missing = "C:\\Nope\\sonara-missing-tts.exe";
    let exe = std::env::current_exe().unwrap().display().to_string();
    std::fs::write(
        h.join(FILE),
        json!({"format": 1, "engines": [
            {"id": "gone", "kind": "command", "options": {"argv": [missing]}},
            {"id": "prog", "kind": "command", "options": {"argv": [exe]}}]})
        .to_string(),
    )
    .unwrap();
    let (e, problems) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    assert!(problems.is_empty(), "{problems:?}");
    let reg = registry();
    e.attach(reg.clone());
    assert!(reg.get("gone").is_ok());
    for (p, replace) in [
        (
            json!({"id": "typo", "kind": "command", "options": {"argv": [missing]}}),
            false,
        ),
        (
            json!({"id": "prog", "kind": "command", "options": {"argv": [exe]}}),
            true,
        ),
        (
            json!({"id": "prog", "kind": "openai-compatible",
                "url": "http://127.0.0.1:9/v1", "key_ref": "none"}),
            true,
        ),
    ] {
        let err = e.add(&p, None, replace).unwrap_err();
        assert_eq!(err.code, Code::Forbidden, "{p}");
    }
    assert_eq!(e.ids(), vec!["gone", "prog"]);
    let list = e.list("prog");
    let view = &list["engines"].as_array().unwrap()[1];
    let name = std::path::Path::new(&exe)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(view["sends_text_to"], format!("program {name}"));
    assert_eq!(view["local"], true);
    assert_eq!(view["key_ref"], "none");
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn keys_of_a_format_1_file_are_bound_to_its_origins_at_load() {
    let h = home();
    let var = format!("SONARA_TEST_LOAD_{}_API_KEY", std::process::id());
    std::fs::write(
        h.join(FILE),
        format!(
            r#"{{"format": 1, "engines": [
            {{"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1",
             "key_ref": "credman"}},
            {{"id": "e", "kind": "openai-compatible", "url": "https://env.example.com/v1",
             "key_ref": "env:{var}"}},
            {{"id": "el", "kind": "elevenlabs", "voice": "abc", "key_ref": "credman"}}]}}"#
        ),
    )
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    store.set_unbound("c", &Secret::new("sk-c"));
    store.set_unbound("el", &Secret::new("sk-el"));
    std::env::set_var(&var, "sk-env");
    let (e, problems) = Engines::load(setup(&h, store.clone()));
    assert!(
        problems.iter().all(|p| !p.contains("bound")),
        "{problems:?}"
    );
    assert_eq!(
        store.get("c").unwrap().unwrap().origin.as_deref(),
        Some("https://tts.example.com:443")
    );
    assert_eq!(
        store.get("el").unwrap().unwrap().origin.as_deref(),
        Some("https://api.elevenlabs.io:443"),
        "a kind this build lacks is bound to its provider"
    );
    assert!(e.get("c").unwrap().key_present());
    assert!(
        e.get("e").unwrap().key_present(),
        "the local file confirms it"
    );
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap();
    assert_eq!(file["format"], FORMAT);
    assert_eq!(
        file["engines"][1]["key_origin"],
        "https://env.example.com:443"
    );
    assert!(file["engines"][0].get("key_origin").is_none());
    // Loaded again (format 2 now), nothing changes.
    let (again, _) = Engines::load(setup(&h, store.clone()));
    assert!(again.get("e").unwrap().key_present());
    std::env::remove_var(&var);
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn a_format_2_file_binds_nothing_new() {
    let h = home();
    let var = format!("SONARA_TEST_LOAD2_{}_API_KEY", std::process::id());
    std::fs::write(
        h.join(FILE),
        format!(
            r#"{{"format": 2, "engines": [
            {{"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1",
             "key_ref": "credman"}},
            {{"id": "e", "kind": "openai-compatible", "url": "https://env.example.com/v1",
             "key_ref": "env:{var}"}},
            {{"id": "ok", "kind": "openai-compatible", "url": "https://env.example.com/v1",
             "key_ref": "env:{var}", "key_origin": "https://env.example.com:443"}}]}}"#
        ),
    )
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    // A key without an origin (an older runtime wrote it after the
    // migration) stays unbound and unused.
    store.set_unbound("c", &Secret::new("sk-c"));
    std::env::set_var(&var, "sk-env");
    let (e, _) = Engines::load(setup(&h, store.clone()));
    assert!(store.get("c").unwrap().unwrap().origin.is_none());
    assert!(!e.get("c").unwrap().key_present());
    assert!(!e.get("e").unwrap().key_present(), "not confirmed");
    assert!(e.get("ok").unwrap().key_present(), "confirmed in the file");
    // A replace over the protocol that keeps the origin keeps the
    // confirmation; one that changes it drops it.
    let mut p = e.view_of("ok", "").unwrap();
    p["voice"] = json!("other");
    e.add(&p, None, true).unwrap();
    assert!(e.get("ok").unwrap().key_present());
    p["url"] = json!("https://evil.example.com/v1");
    e.add(&p, None, true).unwrap();
    assert!(!e.get("ok").unwrap().key_present());
    p["url"] = json!("https://env.example.com/v1");
    e.add(&p, None, true).unwrap();
    assert!(
        !e.get("ok").unwrap().key_present(),
        "a confirmation dropped over the protocol is not restored by it"
    );
    std::env::remove_var(&var);
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn engine_key_binds_to_the_entrys_origin() {
    let h = home();
    std::fs::write(
        h.join(FILE),
        r#"{"format": 2, "engines": [
            {"id": "az", "kind": "azure", "voice": "en-US-VoiceBNeural",
             "options": {"region": "westeurope"}}]}"#,
    )
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    let (e, _) = Engines::load(setup(&h, store.clone()));
    e.set_key("az", Some("k-az")).unwrap();
    assert_eq!(
        store.get("az").unwrap().unwrap().origin.as_deref(),
        Some("https://westeurope.tts.speech.microsoft.com:443")
    );
    let (e, _) = Engines::load(setup(&h, store.clone()));
    e.add(
        &json!({"id": "c", "kind": "openai-compatible", "url": "https://a.example.com/v1"}),
        Some("sk-a"),
        false,
    )
    .unwrap();
    assert_eq!(
        store.get("c").unwrap().unwrap().origin.as_deref(),
        Some("https://a.example.com:443")
    );
    // A replace to another origin without a secret deletes the key.
    e.add(
        &json!({"id": "c", "kind": "openai-compatible", "url": "https://b.example.com/v1"}),
        None,
        true,
    )
    .unwrap();
    assert!(store.get("c").unwrap().is_none());
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn a_broken_file_is_kept_aside() {
    let h = home();
    std::fs::write(h.join(FILE), "{not json").unwrap();
    let (e, problems) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
    assert!(e.ids().is_empty());
    assert!(problems[0].contains("engines.json.bad"));
    assert_eq!(
        std::fs::read_to_string(h.join("engines.json.bad")).unwrap(),
        "{not json"
    );
    let _ = std::fs::remove_dir_all(&h);
}

#[test]
fn notice_lines_follow_the_spec() {
    let n = Notice {
        engine: sonara_engine::EngineId::intern("openai"),
        reason: Some(Reason::Auth),
        status: Some(401),
        message: "OpenAI refused the key (401): bad key sk-abcdefghijklmnopqrstu".into(),
        fallback: Some(sonara_engine::EngineId("kokoro")),
    };
    assert_eq!(
        notice_line(&n),
        "engine openai fallback reason=auth status=401 -> kokoro: OpenAI refused the key \
         (401): bad key [redacted]"
    );
    let r = Notice { reason: None, ..n };
    assert_eq!(notice_line(&r), "engine openai recovered");
}
