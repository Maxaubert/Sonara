//! The `channels` extension (spec 4.2) on top of `sonara_channels`:
//! `channel_open`, `channel_close`, `focus`, `speak` and `control` with a
//! `channel`, `control next_channel`, the `channel_announce` setting and
//! the `state` additions. `protocol` calls in here once a client enabled
//! the extension; before that its messages are `E_UNSUPPORTED`.
use crate::protocol::{bad, opt_str, reader_failure, After, Handled};
use crate::wire::{Code, Failure};
use serde_json::{json, Map, Value};
use sonara_channels::{Channels, Control, Error, ItemId, Policy, QueueMode};
use std::sync::{Arc, OnceLock};

/// The extension's state: empty until a client enables it.
pub type Slot = Arc<OnceLock<Channels>>;

pub const NAME: &str = "channels";

/// The setting that turns switch announcements on and off.
pub const ANNOUNCE_KEY: &str = "channel_announce";

fn failure(e: Error) -> Failure {
    match e {
        Error::UnknownChannel(_) => Failure::new(Code::NotFound, e.to_string()),
        Error::EmptyChannel => Failure::new(Code::BadRequest, e.to_string()),
        Error::Reader(r) => reader_failure(r),
    }
}

fn ok(fields: Map<String, Value>) -> Handled {
    Ok((fields, After::Nothing))
}

/// The required `channel` field: a non-empty string.
fn channel(m: &Map<String, Value>) -> Result<&str, Failure> {
    match opt_str(m, "channel")? {
        Some(c) if !c.is_empty() => Ok(c),
        _ => Err(bad("'channel' must be a non-empty string")),
    }
}

pub fn open(ch: &Channels, m: &Map<String, Value>) -> Handled {
    let id = channel(m)?;
    let label = opt_str(m, "label")?.map(str::to_string);
    let host_tab = opt_str(m, "host_tab")?.map(str::to_string);
    let policy = match opt_str(m, "policy")? {
        None => None,
        Some(p) => Some(Policy::parse(p).ok_or_else(|| bad(format!("unknown policy '{p}'")))?),
    };
    let created = ch.open(id, label, host_tab, policy).map_err(failure)?;
    let policy = ch.channel(id).map(|c| c.policy.as_str());
    let mut f = Map::new();
    f.insert("channel".into(), json!(id));
    f.insert("created".into(), json!(created));
    f.insert("policy".into(), json!(policy));
    ok(f)
}

pub fn close(ch: &Channels, m: &Map<String, Value>) -> Handled {
    ch.close(channel(m)?).map_err(failure)?;
    ok(Map::new())
}

pub fn focus(ch: &Channels, m: &Map<String, Value>) -> Handled {
    ch.focus(channel(m)?).map_err(failure)?;
    ok(Map::new())
}

/// `speak` with a `channel`. `item_id` is null while the text waits in its
/// channel.
pub fn speak(
    ch: &Channels,
    id: &str,
    text: &str,
    mode: Option<QueueMode>,
    interrupt: bool,
    label: Option<String>,
) -> Handled {
    if id.is_empty() {
        return Err(bad("'channel' must be a non-empty string"));
    }
    let spoken = ch
        .speak(id, text, mode, interrupt, label)
        .map_err(failure)?;
    let mut f = Map::new();
    f.insert("item_id".into(), json!(spoken.item_id.map(|i| i.0)));
    f.insert("channel".into(), json!(id));
    f.insert("dropped".into(), json!(spoken.dropped));
    ok(f)
}

/// `control` once the extension is enabled: `next_channel`, or a core
/// action with an optional `channel`.
pub fn control(ch: &Channels, action: Option<Control>, m: &Map<String, Value>) -> Handled {
    let mut f = Map::new();
    match action {
        None => {
            let target = ch.next_channel().map_err(failure)?;
            f.insert("channel".into(), json!(target));
        }
        Some(c) => {
            let id = match opt_str(m, "channel")? {
                Some("") => return Err(bad("'channel' must be a non-empty string")),
                other => other,
            };
            ch.control(c, id).map_err(failure)?;
        }
    }
    ok(f)
}

/// `set channel_announce` (`"on"` or `"off"`) and `get channel_announce`.
pub fn announce(ch: &Channels, value: Option<&Value>) -> Handled {
    if let Some(v) = value {
        match v.as_str() {
            Some("on") => ch.set_announce(true),
            Some("off") => ch.set_announce(false),
            _ => return Err(bad("'channel_announce' is \"on\" or \"off\"")),
        }
    }
    let mut f = Map::new();
    f.insert("key".into(), json!(ANNOUNCE_KEY));
    f.insert(
        "value".into(),
        json!(if ch.announce() { "on" } else { "off" }),
    );
    ok(f)
}

/// Add the extension's fields to a rendered `state` event:
/// `now_playing.channel` and `host_tab` (null for text spoken without a
/// channel) and the channels' unread entries in `queued`.
pub fn annotate_state(ch: &Channels, state: &mut Value) {
    let pending = ch.pending() as u64;
    if let Some(q) = state.get("queued").and_then(Value::as_u64) {
        state["queued"] = json!(q + pending);
    }
    let Some(np) = state.get_mut("now_playing").filter(|v| v.is_object()) else {
        return;
    };
    let tag = np
        .get("item_id")
        .and_then(Value::as_u64)
        .and_then(|id| ch.tag(ItemId(id)));
    np["channel"] = json!(tag.as_ref().map(|t| t.channel.clone()));
    np["host_tab"] = json!(tag.and_then(|t| t.host_tab));
}

#[cfg(test)]
mod tests {
    use crate::lifetime::Lifetime;
    use crate::protocol::{Server, Session};
    use serde_json::{json, Value};
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, ReaderHandle, Registry};
    use std::sync::Arc;
    use std::time::Duration;

    fn server() -> Server {
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        Server::new(
            reader,
            "secret".into(),
            Lifetime::new(Duration::from_secs(30), false),
        )
    }

    fn call(s: &Server, session: &mut Session, req: Value) -> Value {
        s.handle(session, &req).reply
    }

    fn code(r: &Value) -> &str {
        r["error"]["code"].as_str().unwrap_or("")
    }

    /// A server with the extension enabled and a client session.
    fn enabled() -> (Server, Session) {
        let s = server();
        let mut a = Session::tcp();
        call(
            &s,
            &mut a,
            json!({"type": "hello", "token": "secret", "extensions": ["channels"]}),
        );
        (s, a)
    }

    #[test]
    fn the_extension_is_unsupported_until_a_client_enables_it() {
        let s = server();
        let mut a = Session::tcp();
        let r = call(&s, &mut a, json!({"type": "hello", "token": "secret"}));
        assert_eq!(r["extensions"], json!([]));
        let open = json!({"type": "channel_open", "channel": "t1"});
        assert_eq!(code(&call(&s, &mut a, open.clone())), "E_UNSUPPORTED");
        let get = json!({"type": "get", "key": "channel_announce"});
        assert_eq!(code(&call(&s, &mut a, get)), "E_UNSUPPORTED");
        // Another client enables it for the instance.
        let mut b = Session::tcp();
        let r = call(
            &s,
            &mut b,
            json!({"type": "hello", "token": "secret", "extensions": ["channels", "agent"]}),
        );
        assert_eq!(r["extensions"], json!(["channels"]));
        assert_eq!(r["unavailable"], json!(["agent"]));
        let r = call(&s, &mut a, open);
        assert_eq!(r["ok"], true);
        assert_eq!(r["created"], true);
        assert_eq!(r["policy"], "latest");
    }

    #[test]
    fn requiring_the_extension_enables_it() {
        let s = server();
        let mut a = Session::tcp();
        let r = call(
            &s,
            &mut a,
            json!({"type": "hello", "token": "secret", "require": ["channels"]}),
        );
        assert_eq!(r["extensions"], json!(["channels"]));
        assert!(s.channels().is_some());
    }

    #[test]
    fn channel_messages_check_their_fields() {
        let (s, mut a) = enabled();
        let cases = [
            (json!({"type": "channel_open"}), "E_BAD_REQUEST"),
            (
                json!({"type": "channel_open", "channel": ""}),
                "E_BAD_REQUEST",
            ),
            (
                json!({"type": "channel_open", "channel": "a", "policy": "newest"}),
                "E_BAD_REQUEST",
            ),
            (
                json!({"type": "channel_close", "channel": "zz"}),
                "E_NOT_FOUND",
            ),
            (json!({"type": "focus", "channel": "zz"}), "E_NOT_FOUND"),
            (
                json!({"type": "control", "action": "pause", "channel": "zz"}),
                "E_NOT_FOUND",
            ),
            (json!({"type": "control", "action": "fly"}), "E_BAD_REQUEST"),
            (
                json!({"type": "speak", "text": "a", "channel": ""}),
                "E_BAD_REQUEST",
            ),
            (
                json!({"type": "set", "key": "channel_announce", "value": 1}),
                "E_BAD_REQUEST",
            ),
        ];
        for (req, want) in cases {
            let r = call(&s, &mut a, req.clone());
            assert_eq!(code(&r), want, "request {req}");
        }
    }

    #[test]
    fn speak_into_a_channel_and_switch() {
        let (s, mut a) = enabled();
        let r = call(
            &s,
            &mut a,
            json!({"type": "channel_open", "channel": "t1", "label": "One",
                   "host_tab": "tab-1", "policy": "queue"}),
        );
        assert_eq!(r["ok"], true);
        let r = call(
            &s,
            &mut a,
            json!({"type": "speak", "text": "Hello.", "channel": "t1"}),
        );
        assert_eq!(r["item_id"], 1);
        assert_eq!(r["channel"], "t1");
        let r = call(
            &s,
            &mut a,
            json!({"type": "speak", "text": "Again.", "channel": "t1"}),
        );
        assert_eq!(r["item_id"], Value::Null, "waits in its channel");
        let r = call(
            &s,
            &mut a,
            json!({"type": "control", "action": "next_channel"}),
        );
        assert_eq!(r["channel"], "t1");
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "channel_announce", "value": "off"}),
        );
        assert_eq!(r["value"], "off");
        let r = call(
            &s,
            &mut a,
            json!({"type": "get", "key": "channel_announce"}),
        );
        assert_eq!(r["value"], "off");
        let r = call(&s, &mut a, json!({"type": "focus", "channel": "t1"}));
        assert_eq!(r["ok"], true);
        let r = call(&s, &mut a, json!({"type": "control", "action": "stop"}));
        assert_eq!(r["ok"], true);
        let r = call(
            &s,
            &mut a,
            json!({"type": "channel_close", "channel": "t1"}),
        );
        assert_eq!(r["ok"], true);
        assert!(s.is_idle());
    }

    #[test]
    fn state_gains_the_channel_and_host_tab() {
        let (s, mut a) = enabled();
        call(
            &s,
            &mut a,
            json!({"type": "channel_open", "channel": "t1", "host_tab": "tab-1"}),
        );
        call(
            &s,
            &mut a,
            json!({"type": "speak", "text": "Hello.", "channel": "t1"}),
        );
        let ch = s.channels().unwrap();
        let mut state = crate::wire::state_event(&s.reader().state().unwrap(), "fake");
        super::annotate_state(ch, &mut state);
        assert_eq!(state["now_playing"]["channel"], "t1");
        assert_eq!(state["now_playing"]["host_tab"], "tab-1");
        // Text spoken without a channel has none.
        call(&s, &mut a, json!({"type": "control", "action": "stop"}));
        call(&s, &mut a, json!({"type": "speak", "text": "Core."}));
        let mut state = crate::wire::state_event(&s.reader().state().unwrap(), "fake");
        super::annotate_state(ch, &mut state);
        assert_eq!(state["now_playing"]["channel"], Value::Null);
        assert_eq!(state["now_playing"]["host_tab"], Value::Null);
    }
}
