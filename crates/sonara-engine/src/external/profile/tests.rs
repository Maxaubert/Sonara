use super::*;

fn parse(v: Value) -> Result<Profile, ProfileError> {
    Profile::from_json(&v)
}

fn err(v: Value) -> String {
    match parse(v) {
        Err(ProfileError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn local(extra: Value) -> Value {
    let mut v = json!({"id": "loc", "kind": "openai-compatible",
        "url": "http://127.0.0.1:8880/v1", "options": {"preset": "kokoro-fastapi"}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[test]
fn ids_follow_the_rule() {
    for ok in ["a", "openai", "kokoro-gpu", "x_1", &"a".repeat(32)] {
        assert!(validate_id(ok).is_ok(), "{ok}");
    }
    for (bad, msg) in [
        ("", "invalid engine id"),
        ("-a", "invalid engine id"),
        ("A", "invalid engine id"),
        ("a b", "invalid engine id"),
        (&"a".repeat(33), "invalid engine id"),
        ("kokoro", "'kokoro' is a built-in engine"),
        ("onecore", "built-in"),
        ("fake", "built-in"),
        ("sonara-x", "reserved"),
    ] {
        assert!(validate_id(bad).unwrap_err().contains(msg), "{bad}");
    }
    assert!(err(local(json!({"id": "Bad"}))).starts_with("invalid engine id 'Bad': use 1 to 32"));
}

#[test]
fn labels_models_and_voices_are_bounded_plain_text() {
    assert!(err(local(json!({"label": "x".repeat(41)}))).contains("label"));
    assert!(err(local(json!({"label": "a\nb"}))).contains("control"));
    assert!(parse(local(json!({"label": "x".repeat(40)}))).is_ok());
    assert!(err(local(json!({"model": "m".repeat(201)}))).contains("model"));
    assert!(err(local(json!({"voice": "v\u{7}"}))).contains("voice"));
    assert!(err(local(json!({"voice": 3}))).contains("'voice' must be a string"));
}

#[test]
fn urls_need_https_unless_loopback_or_allowed() {
    let cloud = |url: &str, extra: Value| {
        let mut v = json!({"id": "c", "kind": "openai-compatible", "url": url,
            "options": {"preset": "generic"}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    assert!(parse(cloud("https://tts.example.com/v1", json!({}))).is_ok());
    for local in [
        "http://localhost:8880/v1",
        "http://127.0.0.1/v1",
        "http://127.9.9.9:1/v1",
        "http://[::1]:8880/v1",
    ] {
        let p = parse(cloud(local, json!({}))).unwrap();
        assert!(p.is_local(), "{local}");
        assert_eq!(p.key_ref, KeyRef::None, "{local}: local default is no key");
    }
    assert!(err(cloud("http://10.0.0.5:8880/v1", json!({}))).contains("allow_http"));
    // allow_http lets plain http through, but never with a key.
    assert_eq!(
        err(cloud(
            "http://10.0.0.5:8880/v1",
            json!({"options": {"allow_http": true}})
        )),
        "a key is never sent over http to a non-loopback host"
    );
    assert!(parse(cloud(
        "http://10.0.0.5:8880/v1",
        json!({"options": {"allow_http": true}, "key_ref": "none"})
    ))
    .is_ok());
    for bad in [
        "ftp://x/v1",
        "https://user:pw@x/v1",
        "https://x/v1?key=1",
        "https://x/v1#f",
        "x/v1",
        "https://:80/v1",
        // Parser mismatches with the HTTP client (review of #224): the
        // host Sonara checks must be exactly the host ureq connects to.
        "https://[::1]evil.example/v1",
        "http://[::1]evil.example:80/v1",
        "https://[api.openai.com]/v1",
        "https://[]/v1",
        "https://a\\b.example/v1",
        "https://a%2eb.example/v1",
        "https://ex\u{e4}mple.com/v1",
        "https://::1/v1",
        "https://host:/v1",
    ] {
        assert!(err(cloud(bad, json!({}))).contains("invalid url"), "{bad}");
    }
}

#[test]
fn key_refs_and_their_defaults() {
    let openai = parse(json!({"id": "openai", "kind": "openai-compatible",
        "options": {"preset": "openai"}}))
    .unwrap();
    assert_eq!(openai.key_ref, KeyRef::CredMan);
    assert_eq!(
        openai.base_url().as_deref(),
        Some("https://api.openai.com/v1")
    );
    let env = parse(local(json!({"key_ref": "env:MY_1_API_KEY"}))).unwrap();
    assert_eq!(env.key_ref, KeyRef::Env("MY_1_API_KEY".into()));
    assert_eq!(env.to_json()["key_ref"], "env:MY_1_API_KEY");
    assert!(err(local(json!({"key_ref": "env:1BAD"}))).contains("environment variable"));
    // Only a variable named like an API key (or Sonara's own) may be
    // sent: a client must not exfiltrate the runtime's other secrets.
    for ok in [
        "OPENAI_API_KEY",
        "openai_api_key",
        "SONARA_KEY",
        "AZURE_SPEECH_KEY",
        "SPEECH_KEY",
    ] {
        assert!(
            parse(local(json!({"key_ref": format!("env:{ok}")}))).is_ok(),
            "{ok}"
        );
    }
    for bad in ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "PATH", "API_KEY_X"] {
        assert!(
            err(local(json!({"key_ref": format!("env:{bad}")})))
                .contains("must end in _API_KEY or _SPEECH_KEY, or start with SONARA_"),
            "{bad}"
        );
    }
    assert!(err(local(json!({"key_ref": "vault"}))).contains("unknown key_ref"));
    assert_eq!(
        parse(local(json!({"key_ref": null}))).unwrap().key_ref,
        KeyRef::None
    );
}

#[test]
fn options_are_known_and_typed() {
    assert_eq!(
        err(local(
            json!({"options": {"preset": "kokoro-fastapi", "speeed": 1}})
        )),
        "unknown option 'speeed' for kind 'openai-compatible'"
    );
    for (opts, msg) in [
        (json!({"timeout_ms": 999}), "timeout_ms"),
        (json!({"timeout_ms": 120_001}), "timeout_ms"),
        (json!({"prefetch": 0}), "prefetch"),
        (json!({"prefetch": 5}), "prefetch"),
        (json!({"allow_http": "yes"}), "allow_http"),
        (json!({"preset": "acme"}), "unknown preset 'acme'"),
        (json!({"response_format": "mp3"}), "response_format"),
        (json!({"sample_rate": 7999}), "sample_rate"),
        (json!({"instructions": 5}), "instructions"),
        (json!({"extra": [1]}), "extra"),
        (json!({"voices_path": "voices"}), "voices_path"),
    ] {
        let mut o = opts.as_object().unwrap().clone();
        o.entry("preset").or_insert(json!("kokoro-fastapi"));
        assert!(err(local(json!({"options": o}))).contains(msg), "{opts}");
    }
    let ok = parse(local(json!({"options": {"preset": "kokoro-fastapi",
        "timeout_ms": 1000, "prefetch": 4, "allow_http": false, "response_format": "pcm",
        "sample_rate": 24000, "instructions": "calm", "extra": {"stream": false},
        "voices_path": "/audio/voices"}})))
    .unwrap();
    assert_eq!(ok.timeout_ms(), 1000);
    assert_eq!(ok.prefetch(), 4);
}

#[test]
fn preset_requirements_and_defaults() {
    assert!(err(json!({"id": "k", "kind": "openai-compatible",
        "options": {"preset": "kokoro-fastapi"}}))
    .contains("needs a url"));
    // No preset has a default model or voice (#235): a profile without
    // them parses (a stored one stays usable), and a server that needs
    // a model says "choose a model" instead.
    for (preset, required) in [
        ("openai", true),
        ("localai", true),
        ("speaches", true),
        ("kokoro-fastapi", false),
        ("openedai-speech", false),
        ("chatterbox-api", false),
        ("chatterbox-server", false),
        ("generic", false),
    ] {
        let p = parse(json!({"id": "p", "kind": "openai-compatible",
            "url": "http://127.0.0.1:1/v1", "options": {"preset": preset}}))
        .unwrap();
        assert_eq!((p.model.as_deref(), p.voice.as_deref()), (None, None));
        assert_eq!(p.model_required(), required, "{preset}");
        assert_eq!(p.missing_model(), required, "{preset}");
        assert!(p.voice_required() && p.takes_model(), "{preset}");
        let named = parse(json!({"id": "p", "kind": "openai-compatible",
            "url": "http://127.0.0.1:1/v1", "model": "m1", "options": {"preset": preset}}))
        .unwrap();
        assert!(!named.missing_model(), "{preset}");
    }
    // No preset is generic.
    let p = parse(json!({"id": "g", "kind": "openai-compatible",
        "url": "https://tts.example.com/v1/"}))
    .unwrap();
    assert_eq!(p.preset(), Preset::Generic);
    assert_eq!(p.base_url().as_deref(), Some("https://tts.example.com/v1"));
}

#[test]
fn defaults_for_cloud_and_local_profiles() {
    let cloud = parse(json!({"id": "openai", "kind": "openai-compatible",
        "options": {"preset": "openai"}}))
    .unwrap();
    assert_eq!((cloud.timeout_ms(), cloud.prefetch()), (15_000, 2));
    // A whole message adds the time its text takes to speak.
    assert_eq!(cloud.answer_ms(1000), 15_000 + 1000 * ANSWER_MS_PER_CHAR);
    assert!(!cloud.is_local());
    assert_eq!(cloud.sends_text_to(), "api.openai.com");
    assert_eq!(cloud.display_label(), "OpenAI");
    let loc = parse(local(json!({}))).unwrap();
    assert_eq!((loc.timeout_ms(), loc.prefetch()), (60_000, 1));
    assert!(loc.is_local());
    assert_eq!(loc.display_label(), "Kokoro FastAPI");
}

#[test]
fn unknown_kinds_are_unsupported_not_invalid() {
    assert_eq!(
        parse(json!({"id": "fu", "kind": "future-kind", "voice": "x"})),
        Err(ProfileError::Unsupported {
            id: "fu".into(),
            kind: "future-kind".into()
        })
    );
    assert_eq!(implemented_kinds(), Kind::ALL.to_vec());
}

#[test]
fn cartesia_profiles() {
    let ca = |extra: Value| {
        with(
            json!({"id": "ca", "kind": "cartesia", "voice": "v1"}),
            extra,
        )
    };
    let p = parse(ca(json!({}))).unwrap();
    assert_eq!(p.base_url().as_deref(), Some("https://api.cartesia.ai"));
    assert_eq!(p.model, None, "no default model (#235)");
    assert!(p.missing_model(), "Cartesia needs a model in every request");
    assert_eq!(p.key_ref, KeyRef::CredMan);
    assert_eq!(p.display_label(), "Cartesia");
    assert_eq!(p.sends_text_to(), "api.cartesia.ai");
    assert_eq!((p.timeout_ms(), p.prefetch()), (15_000, 2));
    let bare = parse(json!({"id": "ca", "kind": "cartesia"})).unwrap();
    assert_eq!(
        bare.voice, None,
        "no voice yet: parsed, says choose a voice"
    );
    assert!(parse(ca(json!({"model": "m1", "options": {
        "api_version": "2025-04-16", "language": "de", "sample_rate": 44100}})))
    .is_ok());
    for (opts, msg) in [
        (json!({"api_version": "latest"}), "api_version"),
        (json!({"language": "e n"}), "language"),
        (json!({"sample_rate": 12000}), "sample_rate"),
        (
            json!({"region": "x"}),
            "unknown option 'region' for kind 'cartesia'",
        ),
    ] {
        assert!(err(ca(json!({"options": opts}))).contains(msg), "{opts}");
    }
}

#[test]
fn gemini_profiles() {
    let ge = |extra: Value| with(json!({"id": "ge", "kind": "gemini"}), extra);
    let p = parse(ge(json!({}))).unwrap();
    assert_eq!(
        p.base_url().as_deref(),
        Some("https://generativelanguage.googleapis.com")
    );
    assert_eq!(
        p.origin().as_deref(),
        Some("https://generativelanguage.googleapis.com:443")
    );
    // No model and no voice in code (#235): the user picks both from
    // Google's live lists; until then the engine says so.
    assert_eq!((p.model.as_deref(), p.voice.as_deref()), (None, None));
    assert!(p.missing_model() && p.voice_required());
    assert_eq!(p.first_audio_ms(), GEMINI_FIRST_AUDIO_MS);
    assert_eq!(p.key_ref, KeyRef::CredMan);
    assert_eq!(p.display_label(), "Gemini");
    assert_eq!(p.sends_text_to(), "generativelanguage.googleapis.com");
    // One chunk ahead: the free tier's per-minute limit counts the
    // burst at the start of a reply (review of #235).
    // A whole message per request (#235), at most 2000 characters per
    // request; its answer may take as long as the text needs.
    assert_eq!(p.send_mode(), SendMode::Message);
    assert_eq!((p.timeout_ms(), p.prefetch()), (60_000, 1));
    assert_eq!(p.answer_ms(2000), 60_000 + 2000 * ANSWER_MS_PER_CHAR);
    assert_eq!(p.chunk_chars(), Some(2000));
    let set = parse(ge(json!({"model": "m-1.2_x", "voice": "v1",
        "options": {"language_code": "de-DE", "style": "calm and warm",
        "first_audio_ms": 5000}})))
    .unwrap();
    assert!(!set.missing_model());
    assert_eq!(set.first_audio_ms(), 5000);
    // Never longer than the whole timeout.
    let short = parse(ge(
        json!({"options": {"first_audio_ms": 30000, "timeout_ms": 8000}}),
    ))
    .unwrap();
    assert_eq!(short.first_audio_ms(), 8000);
    let max = parse(ge(json!({"options": {"chunk_chars": 4000}}))).unwrap();
    assert_eq!(max.chunk_chars(), Some(4000));
    let own = parse(ge(json!({"options": {"timeout_ms": 20000}}))).unwrap();
    assert_eq!(own.timeout_ms(), 20_000);
    let sentence = parse(ge(json!({"send_mode": "sentence"}))).unwrap();
    assert_eq!(sentence.timeout_ms(), 60_000, "one sentence: a minute");
    // The pre-release options are folded into send_mode: quick_start
    // is dropped, chunk_chars 0 meant one sentence per request.
    let old = parse(ge(
        json!({"options": {"quick_start": false, "chunk_chars": 0}}),
    ))
    .unwrap();
    assert_eq!(old.send_mode(), SendMode::Sentence);
    assert!(old.options.is_empty(), "{:?}", old.options);
    assert_eq!(old.to_json()["send_mode"], "sentence");
    for (extra, msg) in [
        (
            json!({"model": "models/x"}),
            "'model' must be a Gemini model id",
        ),
        (
            json!({"options": {"first_audio_ms": 999}}),
            "first_audio_ms",
        ),
        (
            json!({"options": {"first_audio_ms": 60001}}),
            "first_audio_ms",
        ),
        (json!({"model": "a b"}), "'model'"),
        (json!({"options": {"chunk_chars": 100}}), "chunk_chars"),
        (json!({"options": {"chunk_chars": 5001}}), "chunk_chars"),
        (json!({"options": {"chunk_chars": "big"}}), "chunk_chars"),
        (json!({"options": {"style": "a\nb"}}), "style"),
        (json!({"options": {"style": 5}}), "style"),
        (json!({"options": {"style": "x".repeat(501)}}), "style"),
        (
            json!({"options": {"language_code": "e n"}}),
            "language_code",
        ),
        (
            json!({"options": {"sample_rate": 24000}}),
            "unknown option 'sample_rate' for kind 'gemini'",
        ),
    ] {
        assert!(err(ge(extra.clone())).contains(msg), "{extra}");
    }
    // Other kinds leave the limit to the provider unless asked.
    let g = parse(json!({"id": "g", "kind": "google", "voice": "v1"})).unwrap();
    assert_eq!(g.chunk_chars(), None);
}

/// "Send to the engine" (#235): a whole message for the cloud, a
/// sentence at a time on this PC; explicit in the stored form only when
/// the user chose it.
#[test]
fn send_mode_defaults_per_kind_and_round_trips() {
    let cases = [
        (json!({"id": "a", "kind": "elevenlabs"}), SendMode::Message),
        (
            json!({"id": "a", "kind": "azure", "options": {"region": "westeurope"}}),
            SendMode::Message,
        ),
        (json!({"id": "a", "kind": "google"}), SendMode::Message),
        (json!({"id": "a", "kind": "gemini"}), SendMode::Message),
        (json!({"id": "a", "kind": "cartesia"}), SendMode::Message),
        (json!({"id": "a", "kind": "deepgram"}), SendMode::Message),
        (
            json!({"id": "a", "kind": "openai-compatible", "options": {"preset": "openai"}}),
            SendMode::Message,
        ),
        (
            json!({"id": "a", "kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1",
                "options": {"preset": "kokoro-fastapi"}}),
            SendMode::Sentence,
        ),
        (
            json!({"id": "a", "kind": "openai-compatible", "url": "http://localhost:4123/v1",
                "options": {"preset": "chatterbox-api"}}),
            SendMode::Sentence,
        ),
        (
            json!({"id": "a", "kind": "openai-compatible", "url": "https://tts.example.com/v1",
                "options": {"preset": "generic"}}),
            SendMode::Message,
        ),
    ];
    for (v, mode) in cases {
        let p = parse(v.clone()).unwrap();
        assert_eq!(p.send_mode(), mode, "{v}");
        assert_eq!(p.default_send_mode(), mode, "{v}");
        assert!(
            p.to_json().get("send_mode").is_none(),
            "a default is not stored"
        );
    }
    // The user's choice wins and is stored.
    let p = parse(json!({"id": "a", "kind": "elevenlabs", "send_mode": "sentence"})).unwrap();
    assert_eq!(p.send_mode(), SendMode::Sentence);
    assert_eq!(p.to_json()["send_mode"], "sentence");
    assert_eq!(parse(p.to_json()).unwrap(), p);
    assert_eq!(p.timeout_ms(), 15_000, "a sentence keeps the short wait");
    assert_eq!(p.answer_ms(1000), 15_000, "no time per character");
    let m = parse(json!({"id": "a", "kind": "elevenlabs"})).unwrap();
    assert_eq!(m.timeout_ms(), 15_000);
    assert_eq!(m.answer_ms(100), 15_000 + 100 * ANSWER_MS_PER_CHAR);
    // Every kind takes chunk_chars and first_audio_ms now.
    let c = parse(json!({"id": "a", "kind": "cartesia",
        "options": {"chunk_chars": 800, "first_audio_ms": 3000}}))
    .unwrap();
    assert_eq!((c.chunk_chars(), c.first_audio_ms()), (Some(800), 3000));
    for bad in [json!("whole"), json!(1), json!(true)] {
        let e = err(json!({"id": "a", "kind": "elevenlabs", "send_mode": bad}));
        assert!(e.contains("'send_mode' must be"), "{e}");
    }
    let null = parse(json!({"id": "a", "kind": "elevenlabs", "send_mode": null})).unwrap();
    assert_eq!(null.send_mode, None);
}

#[test]
fn deepgram_profiles() {
    let dg = |extra: Value| {
        with(
            json!({"id": "dg", "kind": "deepgram", "voice": "v1"}),
            extra,
        )
    };
    let p = parse(dg(json!({}))).unwrap();
    assert_eq!(p.base_url().as_deref(), Some("https://api.deepgram.com"));
    assert_eq!(p.model, None);
    assert!(!p.takes_model() && !p.missing_model());
    assert_eq!(p.key_ref, KeyRef::CredMan);
    assert_eq!(p.display_label(), "Deepgram");
    let eu = parse(dg(json!({"url": "https://api.eu.deepgram.com"}))).unwrap();
    assert_eq!(eu.sends_text_to(), "api.eu.deepgram.com");
    assert!(parse(json!({"id": "dg", "kind": "deepgram"})).is_ok());
    assert!(err(dg(json!({"model": "m1"}))).contains("the voice is the model"));
    assert!(parse(dg(json!({"options": {"sample_rate": 48000}}))).is_ok());
    assert!(err(dg(json!({"options": {"sample_rate": 22050}}))).contains("sample_rate"));
}

fn exe() -> String {
    // A real .exe that exists on every Windows (check_new).
    std::env::var("SystemRoot")
        .map(|r| format!("{r}\\System32\\whoami.exe"))
        .unwrap_or_else(|_| "C:\\Windows\\System32\\whoami.exe".into())
}

#[test]
fn command_profiles() {
    let cmd = |options: Value| json!({"id": "cmd", "kind": "command", "options": options});
    let p = parse(cmd(json!({"argv": [exe(), "--voice", "{voice}"],
        "voices": ["amy", "joe"]})))
    .unwrap();
    assert_eq!(p.key_ref, KeyRef::None, "a program needs no key by default");
    assert_eq!(p.sends_text_to(), "program whoami.exe");
    assert_eq!(p.host(), "");
    assert!(p.is_local());
    assert_eq!((p.timeout_ms(), p.prefetch()), (30_000, 1));
    assert_eq!(p.display_label(), "The speech program");
    assert_eq!(
        p.command_argv(),
        vec![exe(), "--voice".into(), "{voice}".into()]
    );
    assert!(p.check_new().is_ok());
    let gone = parse(cmd(json!({"argv": ["C:\\Nope\\missing-tts.exe"]}))).unwrap();
    assert_eq!(
        gone.check_new(),
        Err(ProfileError::Invalid(
            "the program 'C:\\Nope\\missing-tts.exe' does not exist".into()
        ))
    );
    for (opts, msg) in [
        (json!({}), "needs options.argv"),
        (json!({"argv": []}), "needs options.argv"),
        (json!({"argv": "tts.exe"}), "needs options.argv"),
        (json!({"argv": ["tts.exe"]}), "full path"),
        (json!({"argv": ["{text}.exe"]}), "full path"),
        (json!({"argv": ["C:\\Tools\\say.bat"]}), "must be an .exe"),
        (json!({"argv": ["C:\\Tools\\say.cmd"]}), "must be an .exe"),
        (json!({"argv": ["C:\\Tools\\tts.exe", 3]}), "list of texts"),
        // The text never goes on the command line (security review of
        // PR3): stdin, or a temporary file at {in}.
        (
            json!({"argv": [exe(), "--say", "{text}"]}),
            "{text} is not allowed",
        ),
        (
            json!({"argv": [exe(), "--say={text}"], "input": "arg"}),
            "{text} is not allowed",
        ),
        (json!({"argv": [exe()], "input": "arg"}), "'input'"),
        (json!({"argv": [exe()], "input": "pipe"}), "'input'"),
        (json!({"argv": [exe()], "input": "file"}), "needs {in}"),
        (
            json!({"argv": [exe(), "{in}"]}),
            "needs option input 'file'",
        ),
        (json!({"argv": [exe()], "output": "file"}), "needs {out}"),
        (
            json!({"argv": [exe(), "{out}"]}),
            "needs option output 'file'",
        ),
        (
            json!({"argv": [exe()], "output": "stdout-pcm"}),
            "sample_rate",
        ),
        (json!({"argv": [exe()], "output": "mp3"}), "'output'"),
        (json!({"argv": [exe()], "voices": "amy"}), "voices"),
        (json!({"argv": [exe()], "voices": [""]}), "voices"),
        (
            json!({"argv": [exe()], "preset": "x"}),
            "unknown option 'preset'",
        ),
    ] {
        assert!(err(cmd(opts.clone())).contains(msg), "{opts}");
    }
    let many: Vec<String> = (0..65).map(|_| exe()).collect();
    assert!(err(cmd(json!({"argv": many}))).contains("more than 64"));
    assert!(
        parse(cmd(json!({"argv": [exe(), "-t", "{in}", "-o", "{out}"],
        "input": "file", "output": "file"})))
        .is_ok()
    );
    assert!(parse(cmd(json!({"argv": [exe()], "output": "stdout-pcm",
        "sample_rate": 22050})))
    .is_ok());
    let mut v = cmd(json!({"argv": [exe()]}));
    v["url"] = json!("http://127.0.0.1:1");
    assert!(err(v).contains("takes no url"));
    let mut v = cmd(json!({"argv": [exe()]}));
    v["model"] = json!("m");
    assert!(err(v).contains("takes no model"));
    assert_eq!(program_name("C:\\Tools\\piper\\piper.exe"), "piper.exe");
    assert_eq!(program_name("/x/y.exe"), "y.exe");
}

#[test]
fn origins_include_the_port_and_the_kind_defaults() {
    let o = |v: Value| origin_of(&v);
    assert_eq!(
        o(json!({"kind": "openai-compatible", "options": {"preset": "openai"}})).as_deref(),
        Some("https://api.openai.com:443")
    );
    assert_eq!(
        o(json!({"kind": "openai-compatible", "url": "https://API.example.com/v1/"})).as_deref(),
        Some("https://api.example.com:443")
    );
    assert_eq!(
        o(json!({"kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1"})).as_deref(),
        Some("http://127.0.0.1:8880")
    );
    assert_eq!(
        o(json!({"kind": "openai-compatible", "url": "http://[::1]/v1"})).as_deref(),
        Some("http://[::1]:80")
    );
    assert_eq!(
        o(json!({"kind": "openai-compatible", "url": "https://x.example:8443"})).as_deref(),
        Some("https://x.example:8443")
    );
    // Kinds this build lacks still have an origin (their keys are
    // bound too): the provider's default or Azure's region.
    for (kind, want) in [
        ("elevenlabs", "https://api.elevenlabs.io:443"),
        ("google", "https://texttospeech.googleapis.com:443"),
        ("gemini", "https://generativelanguage.googleapis.com:443"),
        ("cartesia", "https://api.cartesia.ai:443"),
        ("deepgram", "https://api.deepgram.com:443"),
    ] {
        assert_eq!(o(json!({"kind": kind})).as_deref(), Some(want), "{kind}");
    }
    assert_eq!(
        o(json!({"kind": "azure", "options": {"region": "westeurope"}})).as_deref(),
        Some("https://westeurope.tts.speech.microsoft.com:443")
    );
    assert_eq!(
        o(json!({"kind": "azure", "options": {"region": "evil.example.com/x"}})),
        None,
        "a region is a host label, not an address"
    );
    assert_eq!(
        o(json!({"kind": "deepgram", "url": "https://api.eu.deepgram.com"})).as_deref(),
        Some("https://api.eu.deepgram.com:443")
    );
    assert_eq!(o(json!({"kind": "command"})), None);
    assert_eq!(
        o(json!({"kind": "command", "options": {"argv": ["C:\\Tools\\TTS.exe", "-v"]}})).as_deref(),
        Some("command:c:\\tools\\tts.exe"),
        "a command is bound to its program"
    );
    assert_eq!(
        o(json!({"kind": "openai-compatible", "url": "ftp://x"})),
        None
    );
    // A profile's origin and its provider default.
    let p = parse(json!({"id": "openai", "kind": "openai-compatible",
        "url": "https://proxy.example.com/v1", "options": {"preset": "openai"}}))
    .unwrap();
    assert_eq!(p.origin().as_deref(), Some("https://proxy.example.com:443"));
    assert_eq!(
        p.default_origin().as_deref(),
        Some("https://api.openai.com:443")
    );
    assert_eq!(parse(local(json!({}))).unwrap().default_origin(), None);
    assert_eq!(
        parse(
            json!({"id": "k", "kind": "openai-compatible", "key_origin": "https://evil:443",
            "url": "https://tts.example.com/v1"})
        )
        .unwrap()
        .key_origin,
        None,
        "key_origin never comes from the profile JSON"
    );
}

#[test]
fn json_round_trip_drops_secret_fields() {
    let v = json!({"id": "openai", "kind": "openai-compatible", "label": "OpenAI",
        "url": "https://api.openai.com/v1", "model": "m1", "voice": "v1",
        "key_ref": "credman", "options": {"preset": "openai"},
        "secret": "sk-should-never-be-kept-1234", "api_key": "x"});
    let p = parse(v).unwrap();
    let out = p.to_json();
    assert!(out.get("secret").is_none() && out.get("api_key").is_none());
    assert_eq!(parse(out.clone()).unwrap(), p);
    assert_eq!(
        out,
        json!({"id": "openai", "kind": "openai-compatible", "label": "OpenAI",
            "url": "https://api.openai.com/v1", "model": "m1", "voice": "v1",
            "key_ref": "credman", "options": {"preset": "openai"}})
    );
}

fn with(base: Value, extra: Value) -> Value {
    let mut v = base;
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[test]
fn elevenlabs_profiles() {
    let el = |extra: Value| {
        with(
            json!({"id": "el", "kind": "elevenlabs", "voice": "v1"}),
            extra,
        )
    };
    let p = parse(el(json!({}))).unwrap();
    assert_eq!(p.base_url().as_deref(), Some("https://api.elevenlabs.io"));
    // The model is optional: ElevenLabs picks its own when none is sent.
    assert_eq!(p.model, None);
    assert!(p.takes_model() && !p.model_required() && !p.missing_model());
    assert_eq!(p.key_ref, KeyRef::CredMan);
    assert_eq!(p.display_label(), "ElevenLabs");
    assert_eq!(p.sends_text_to(), "api.elevenlabs.io");
    assert_eq!((p.timeout_ms(), p.prefetch()), (15_000, 2));
    assert!(parse(json!({"id": "el", "kind": "elevenlabs"})).is_ok());
    // A cloud kind keeps credman even behind a loopback url.
    let proxy = parse(el(json!({"url": "http://127.0.0.1:9"}))).unwrap();
    assert_eq!(proxy.key_ref, KeyRef::CredMan);
    assert!(parse(el(json!({"options": {"output_format": "pcm_44100",
        "stability": 0.5, "similarity_boost": 1, "style": 0, "language_code": "de",
        "enable_logging": false}})))
    .is_ok());
    for (opts, msg) in [
        (json!({"output_format": "mp3_44100_128"}), "output_format"),
        (json!({"stability": 1.5}), "stability"),
        (json!({"similarity_boost": "high"}), "similarity_boost"),
        (json!({"style": -0.1}), "style"),
        (json!({"language_code": "e n"}), "language_code"),
        (json!({"enable_logging": "no"}), "enable_logging"),
        (
            json!({"region": "x"}),
            "unknown option 'region' for kind 'elevenlabs'",
        ),
    ] {
        assert!(err(el(json!({"options": opts}))).contains(msg), "{opts}");
    }
}

#[test]
fn azure_profiles() {
    let az = |extra: Value| {
        with(
            json!({"id": "az", "kind": "azure", "voice": "v1",
                "options": {"region": "westeurope"}}),
            extra,
        )
    };
    let p = parse(az(json!({}))).unwrap();
    assert_eq!(
        p.base_url().as_deref(),
        Some("https://westeurope.tts.speech.microsoft.com")
    );
    assert_eq!(p.sends_text_to(), "westeurope.tts.speech.microsoft.com");
    assert_eq!(p.display_label(), "Azure Speech");
    assert_eq!(p.key_ref, KeyRef::CredMan);
    let by_url = parse(json!({"id": "az", "kind": "azure", "voice": "v1",
        "url": "https://eastus.tts.speech.microsoft.com/"}))
    .unwrap();
    assert_eq!(
        by_url.base_url().as_deref(),
        Some("https://eastus.tts.speech.microsoft.com")
    );
    assert_eq!(
        err(json!({"id": "az", "kind": "azure", "voice": "v1"})),
        "kind 'azure' needs options.region or url"
    );
    assert!(parse(json!({"id": "az", "kind": "azure", "options": {"region": "eastus"}})).is_ok());
    assert_eq!(
        err(az(json!({"model": "x"}))),
        "kind 'azure' takes no model"
    );
    assert!(parse(az(json!({"options": {"region": "eastus2",
        "output_format": "raw-48khz-16bit-mono-pcm", "lang": "en-GB"}})))
    .is_ok());
    for (opts, msg) in [
        (json!({"region": "West Europe"}), "region"),
        (
            json!({"region": "westeurope", "output_format": "riff-24khz-16bit-mono-pcm"}),
            "output_format",
        ),
        (json!({"region": "westeurope", "lang": "<x>"}), "lang"),
    ] {
        assert!(err(az(json!({"options": opts}))).contains(msg), "{opts}");
    }
}

#[test]
fn google_profiles() {
    let g = |extra: Value| with(json!({"id": "g", "kind": "google", "voice": "v1"}), extra);
    let p = parse(g(json!({}))).unwrap();
    assert_eq!(
        p.base_url().as_deref(),
        Some("https://texttospeech.googleapis.com")
    );
    assert_eq!(p.display_label(), "Google Text-to-Speech");
    assert_eq!(p.key_ref, KeyRef::CredMan);
    assert!(err(g(json!({"model": "gemini"}))).contains("options.model_name"));
    assert!(parse(g(
        json!({"options": {"language_code": "en-US", "sample_rate": 16000,
        "user_project": "my-project-1", "model_name": "m-1.0_x"}})
    ))
    .is_ok());
    for (opts, msg) in [
        (json!({"sample_rate": 96000}), "sample_rate"),
        (json!({"user_project": "a b"}), "user_project"),
        (json!({"language_code": ""}), "language_code"),
        (json!({"model_name": "x?y"}), "model_name"),
    ] {
        assert!(err(g(json!({"options": opts}))).contains(msg), "{opts}");
    }
}

#[test]
fn locales_come_from_voice_names() {
    assert_eq!(voice_locale("en-US-VoiceANeural").as_deref(), Some("en-US"));
    assert_eq!(voice_locale("en-US-Voice-A").as_deref(), Some("en-US"));
    assert_eq!(voice_locale("cmn-CN-Wavenet-A").as_deref(), Some("cmn-CN"));
    assert_eq!(
        voice_locale("zh-CN-henan-YundengNeural").as_deref(),
        Some("zh-CN")
    );
    assert_eq!(voice_locale("VoiceA"), None);
    assert_eq!(voice_locale("en-US"), None);
    assert_eq!(voice_locale("voiceid0000000000001"), None);
}
