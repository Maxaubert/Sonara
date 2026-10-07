use super::*;
use sonara_audio::TestOutput;
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Registry};
use std::time::Duration;

fn server() -> (Server, TestOutput) {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx)).unwrap();
    let life = Lifetime::new(Duration::from_secs(30), false);
    (Server::new(reader, "secret".into(), life), out)
}

fn call(s: &Server, session: &mut Session, req: Value) -> Outcome {
    s.handle(session, &req)
}

fn authed(s: &Server) -> Session {
    let mut session = Session::tcp();
    let o = call(s, &mut session, json!({"type": "hello", "token": "secret"}));
    assert_eq!(o.reply["ok"], true);
    session
}

fn code(o: &Outcome) -> &str {
    o.reply["error"]["code"].as_str().unwrap_or("")
}

#[test]
fn every_extension_type_is_unsupported_until_its_extension_is_enabled() {
    let (s, _) = server();
    let mut session = authed(&s);
    let all = [
        "channel_open",
        "channel_close",
        "focus",
        "stream",
        "turn_start",
        "turn_end",
        "ask",
        "earcon",
        "tool",
        "answered",
        "preview",
        "shutdown",
    ];
    for kind in all {
        let o = call(&s, &mut session, json!({"type": kind}));
        assert_eq!(code(&o), "E_UNSUPPORTED", "{kind}");
    }
    let known: Vec<&str> = EXTENSION_TYPES
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();
    assert_eq!(known, all);
    let o = call(&s, &mut session, json!({"type": "no_such_type"}));
    assert_eq!(code(&o), "E_UNKNOWN_TYPE");
}

#[test]
fn tcp_needs_hello_with_the_token_first() {
    let (s, _) = server();
    let mut session = Session::tcp();
    let o = call(&s, &mut session, json!({"type": "speak", "text": "Hi."}));
    assert_eq!(code(&o), "E_AUTH");
    assert!(matches!(o.after, After::Close));
    let o = call(&s, &mut session, json!({"type": "hello", "token": "nope"}));
    assert_eq!(code(&o), "E_AUTH");
    assert!(!session.authed);
    let o = call(&s, &mut session, json!("not an object"));
    assert_eq!(code(&o), "E_AUTH");
}

#[test]
fn hello_reports_version_protocol_and_capabilities() {
    let (s, _) = server();
    let mut session = Session::tcp();
    let o = call(
        &s,
        &mut session,
        json!({"type": "hello", "token": "secret", "id": "h1",
               "client": {"name": "t", "version": "1"},
               "protocol": {"major": 1, "minor": 0},
               "extensions": ["channels"], "future_field": 1}),
    );
    let r = &o.reply;
    assert_eq!(r["id"], "h1");
    assert_eq!(r["version"], crate::VERSION);
    assert_eq!(r["protocol"], json!({"major": 1, "minor": 6}));
    assert!(r["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("core")));
    assert_eq!(r["extensions"], json!(["channels"]));
    assert_eq!(r["unavailable"], json!([]));
    assert!(session.authed);
}

#[test]
fn hello_rejects_an_unmet_require_and_another_major() {
    let (s, _) = server();
    let mut session = Session::tcp();
    let o = call(
        &s,
        &mut session,
        json!({"type": "hello", "token": "secret", "require": ["core", "system"]}),
    );
    assert_eq!(code(&o), "E_UNSUPPORTED");
    assert!(!session.authed);
    let o = call(
        &s,
        &mut session,
        json!({"type": "hello", "token": "secret", "protocol": {"major": 2, "minor": 0}}),
    );
    assert_eq!(code(&o), "E_INCOMPATIBLE");
}

#[test]
fn takeover_is_accepted_only_when_idle() {
    let (s, out) = server();
    let mut session = authed(&s);
    call(
        &s,
        &mut session,
        json!({"type": "speak", "text": "One. Two."}),
    );
    let mut other = Session::tcp();
    let hello = json!({"type": "hello", "token": "secret", "takeover": true});
    let o = call(&s, &mut other, hello.clone());
    assert_eq!(code(&o), "E_BUSY");
    call(
        &s,
        &mut session,
        json!({"type": "control", "action": "pause"}),
    );
    let o = call(&s, &mut other, hello.clone());
    assert_eq!(code(&o), "E_BUSY", "a paused item is not idle");
    call(
        &s,
        &mut session,
        json!({"type": "control", "action": "stop"}),
    );
    let _ = out.take_calls();
    let o = call(&s, &mut other, hello);
    assert_eq!(o.reply["ok"], true);
    assert_eq!(o.reply["takeover"], true);
    assert!(matches!(o.after, After::Exit));
}

#[test]
fn after_an_accepted_takeover_speak_and_control_are_busy() {
    let (s, _) = server();
    let mut session = authed(&s);
    let mut other = Session::tcp();
    let o = call(
        &s,
        &mut other,
        json!({"type": "hello", "token": "secret", "takeover": true}),
    );
    assert_eq!(o.reply["ok"], true);
    let o = call(&s, &mut session, json!({"type": "speak", "text": "Late."}));
    assert_eq!(
        code(&o),
        "E_BUSY",
        "a speak after the takeover would be dropped"
    );
    let o = call(
        &s,
        &mut session,
        json!({"type": "control", "action": "play"}),
    );
    assert_eq!(code(&o), "E_BUSY");
    assert!(s.is_idle());
}

#[test]
fn unknown_types_extension_types_and_bad_fields() {
    let (s, _) = server();
    let mut session = authed(&s);
    let o = call(&s, &mut session, json!({"type": "dance", "id": 3}));
    assert_eq!(code(&o), "E_UNKNOWN_TYPE");
    assert_eq!(o.reply["id"], 3);
    let o = call(
        &s,
        &mut session,
        json!({"type": "channel_open", "channel": "a"}),
    );
    assert_eq!(code(&o), "E_UNSUPPORTED");
    let o = call(&s, &mut session, json!({"type": "speak"}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = call(
        &s,
        &mut session,
        json!({"type": "speak", "text": "a", "mode": "x"}),
    );
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = call(
        &s,
        &mut session,
        json!({"type": "control", "action": "fly"}),
    );
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = call(
        &s,
        &mut session,
        json!({"type": "control", "action": "next_channel"}),
    );
    assert_eq!(code(&o), "E_UNSUPPORTED");
    let o = call(
        &s,
        &mut session,
        json!({"type": "set", "key": "volume", "value": 101}),
    );
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = call(
        &s,
        &mut session,
        json!({"type": "set", "key": "voice", "value": "nobody"}),
    );
    assert_eq!(code(&o), "E_NOT_FOUND");
    let o = call(
        &s,
        &mut session,
        json!({"type": "get", "key": "audio_mode"}),
    );
    assert_eq!(code(&o), "E_UNSUPPORTED");
    let o = call(
        &s,
        &mut session,
        json!({"type": "voices", "engine": "nope"}),
    );
    assert_eq!(code(&o), "E_NOT_FOUND");
    let o = call(&s, &mut session, json!({"no": "type"}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
}

#[test]
fn speak_set_get_and_voices() {
    let (s, _) = server();
    let mut session = authed(&s);
    let o = call(
        &s,
        &mut session,
        json!({"type": "speak", "text": "Hello.", "label": "x", "extra": [1]}),
    );
    assert_eq!(o.reply["item_id"], 1);
    let o = call(
        &s,
        &mut session,
        json!({"type": "set", "key": "volume", "value": 40}),
    );
    assert_eq!(o.reply["value"], 40);
    let o = call(&s, &mut session, json!({"type": "get", "key": "engine"}));
    assert_eq!(o.reply["value"], "fake");
    let o = call(&s, &mut session, json!({"type": "voices"}));
    let voices = o.reply["voices"].as_array().unwrap();
    assert_eq!(voices[0]["engine"], "fake");
    assert_eq!(voices[0]["license_class"], "permissive");
}

#[test]
fn subscribe_is_tcp_only_and_checks_names() {
    let (s, _) = server();
    let mut http = Session::http();
    let o = call(
        &s,
        &mut http,
        json!({"type": "subscribe", "events": ["state"]}),
    );
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let mut session = authed(&s);
    let o = call(
        &s,
        &mut session,
        json!({"type": "subscribe", "events": ["bogus"]}),
    );
    assert_eq!(code(&o), "E_UNSUPPORTED");
    let o = call(
        &s,
        &mut session,
        json!({"type": "subscribe", "events": ["state"]}),
    );
    assert_eq!(o.reply["events"], json!(["state"]));
    let After::Subscribe(mut rx) = o.after else {
        panic!("expected a subscription");
    };
    let first = rx.blocking_recv().unwrap();
    assert_eq!(first.name, "state");
    let v: Value = serde_json::from_str(&first.json).unwrap();
    assert_eq!(v["engine_status"]["engine"], "fake");
}

#[test]
fn keep_alive_in_hello_is_sticky() {
    let (s, _) = server();
    let mut session = Session::tcp();
    call(
        &s,
        &mut session,
        json!({"type": "hello", "token": "secret", "keep_alive": true}),
    );
    assert!(!s.lifetime().may_idle_exit());
}

#[test]
fn an_idle_exit_is_refused_while_reading_and_then_admits_nothing() {
    // #194: the exit decision and a request that starts speech must
    // not interleave (sonarad exited while an item was playing).
    let (s, out) = server();
    let life_idle = Lifetime::new(Duration::ZERO, false);
    let s = Server {
        lifetime: life_idle,
        ..s
    };
    let mut session = Session::http();
    call(
        &s,
        &mut session,
        json!({"type": "speak", "text": "One. Two."}),
    );
    assert!(!s.retire_if_idle(), "reading: no exit");
    assert_eq!(
        code(&call(
            &s,
            &mut session,
            json!({"type": "control", "action": "stop"})
        )),
        ""
    );
    let _ = out.take_calls();
    assert!(s.retire_if_idle(), "idle and expired: exit");
    let o = call(&s, &mut session, json!({"type": "speak", "text": "Late."}));
    assert_eq!(code(&o), "E_BUSY", "nothing is accepted and then dropped");
}

#[test]
fn a_recent_request_keeps_it_from_retiring() {
    let (s, _) = server();
    let s = Server {
        lifetime: Lifetime::new(Duration::from_secs(30), false),
        ..s
    };
    s.lifetime().touch();
    assert!(!s.retire_if_idle());
    s.lifetime().set_keep_alive();
    assert!(!s.retire_if_idle());
}

#[test]
fn token_comparison() {
    assert!(token_eq("abc", "abc"));
    assert!(!token_eq("abc", "abd"));
    assert!(!token_eq("abc", "abcd"));
}
