//! The `channels` extension (spec 4.2) on top of `sonara_channels`:
//! `channel_open`, `channel_close`, `focus`, `speak` and `control` with a
//! `channel`, `control next_channel`, the `channel_announce` setting and
//! the `state` additions. `protocol` calls in here once a client enabled
//! the extension; before that its messages are `E_UNSUPPORTED`.
use crate::config::{PrefsUpdate, Store};
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

/// The per-channel preferences (label, voice, muted) of the settings page.
pub const PREFS_KEY: &str = "channel_prefs";

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

/// `channel_open`. A label the user gave the channel (`channel_prefs`)
/// replaces the client's; the client's is remembered for the page.
pub fn open(ch: &Channels, store: &Store, m: &Map<String, Value>) -> Handled {
    let id = channel(m)?;
    let client_label = opt_str(m, "label")?;
    store.note_client_label(id, client_label);
    let label = store
        .prefs(id)
        .label
        .or_else(|| client_label.map(str::to_string));
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

/// A channel opened implicitly (by its first text) with a label the user
/// gave it: open it with that label first, so switches announce it.
pub fn apply_label(ch: &Channels, store: &Store, id: &str) {
    if id.is_empty() {
        return;
    }
    if let Some(label) = store.prefs(id).label {
        if !ch.channel_ids().iter().any(|c| c == id) {
            let _ = ch.open(id, Some(label), None, None);
        }
    }
}

/// The page's list: open channels in opening order, then channels with
/// stored preferences only, the most recently changed first.
fn prefs_list(ch: &Channels, store: &Store) -> Value {
    let reading = ch.engaged();
    let mut rows: Vec<Value> = Vec::new();
    let open = ch.channel_ids();
    for id in &open {
        let c = ch.channel(id);
        let p = store.prefs(id);
        rows.push(json!({
            "channel": id,
            "open": true,
            "reading": reading.as_deref() == Some(id.as_str()),
            "client_label": store.client_label(id).or_else(|| c.as_ref().and_then(|c| c.label.clone())),
            "host_tab": c.and_then(|c| c.host_tab),
            "label": p.label,
            "voice": p.voice,
            "muted": p.muted,
        }));
    }
    for (id, p) in store.all_prefs().into_iter().rev() {
        if open.contains(&id) {
            continue;
        }
        rows.push(json!({
            "channel": id,
            "open": false,
            "reading": false,
            "client_label": store.client_label(&id),
            "host_tab": null,
            "label": p.label,
            "voice": p.voice,
            "muted": p.muted,
        }));
    }
    Value::Array(rows)
}

/// An optional text field of a prefs update: absent leaves it, `null` or
/// `""` clears it.
fn text_update(m: &Map<String, Value>, field: &str) -> Result<Option<Option<String>>, Failure> {
    match m.get(field) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
        Some(_) => Err(bad(format!(
            "'channel_prefs.{field}' must be a string or null"
        ))),
    }
}

/// Mute every channel the user muted (a new `Channels`).
pub fn apply_mutes(ch: &Channels, store: &Store) {
    for (id, p) in store.all_prefs() {
        if p.muted {
            let _ = ch.set_muted(&id, true);
        }
    }
}

/// `get channel_prefs` (the list) and `set channel_prefs {channel, label?,
/// voice?, muted?}` (stored at once; a new label renames an open channel's
/// announcements; `muted` holds the channel's speech unread at once, #196;
/// the voice is stored for the page). `set channel_prefs {channel, forget:
/// true}` forgets a channel that is not focused: its preferences, and its
/// channel and turn state (`forget` closes it when the agent is on).
pub fn prefs_setting(
    ch: &Channels,
    agent: Option<&sonara_agent::Agent>,
    store: &Store,
    value: Option<&Value>,
) -> Handled {
    if let Some(v) = value {
        let m = v.as_object().ok_or_else(|| {
            bad("'channel_prefs' is {channel, label?, voice?, muted?} or {channel, forget: true}")
        })?;
        let id = channel(m)?;
        if m.get("forget") == Some(&Value::Bool(true)) {
            if ch.focused().as_deref() == Some(id) {
                return Err(bad("the focused channel cannot be forgotten"));
            }
            store.forget_prefs(id);
            ch.set_muted(id, false).map_err(failure)?;
            match agent {
                Some(a) => a.forget(id).map_err(crate::agent_ext::failure)?,
                None if ch.channel(id).is_some() => ch.close(id).map_err(failure)?,
                None => {}
            }
            let mut f = Map::new();
            f.insert("key".into(), json!(PREFS_KEY));
            f.insert("value".into(), prefs_list(ch, store));
            return ok(f);
        }
        let muted = match m.get("muted") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(bad("'channel_prefs.muted' must be true or false")),
        };
        let update = PrefsUpdate {
            label: text_update(m, "label")?,
            voice: text_update(m, "voice")?,
            muted,
        };
        let relabel = update.label.is_some();
        let p = store.set_prefs(id, update);
        if let Some(mute) = muted {
            ch.set_muted(id, mute).map_err(failure)?;
        }
        if relabel {
            if let Some(c) = ch.channel(id) {
                let label = p.label.or_else(|| store.client_label(id));
                let _ = ch.open(id, label, c.host_tab, None);
            }
        }
    }
    let mut f = Map::new();
    f.insert("key".into(), json!(PREFS_KEY));
    f.insert("value".into(), prefs_list(ch, store));
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
            json!({"type": "hello", "token": "secret", "extensions": ["channels", "system"]}),
        );
        assert_eq!(r["extensions"], json!(["channels"]));
        assert_eq!(r["unavailable"], json!(["system"]));
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
