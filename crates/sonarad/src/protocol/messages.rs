//! The core messages (spec 4.1): `hello`, `speak`, `control`, `set`, `get`,
//! `voices`, `subscribe` and `shutdown`.
use super::*;

impl Server {
    pub(super) fn hello_fields(&self, unavailable: Vec<&str>) -> Map<String, Value> {
        let mut f = Map::new();
        f.insert("version".into(), json!(crate::VERSION));
        f.insert(
            "protocol".into(),
            json!({"major": PROTOCOL_MAJOR, "minor": PROTOCOL_MINOR}),
        );
        f.insert("capabilities".into(), json!(self.capabilities()));
        f.insert("extensions".into(), json!(self.enabled()));
        f.insert("unavailable".into(), json!(unavailable));
        f
    }

    pub(super) fn hello(&self, session: &mut Session, m: &Map<String, Value>) -> Handled {
        if !session.authed {
            let token = m.get("token").and_then(Value::as_str).unwrap_or("");
            if !self.token_ok(token) {
                return Err(Failure::new(Code::Auth, "wrong or missing token"));
            }
        }
        if opt_bool(m, "takeover")? {
            let mut retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
            if !*retiring && !self.is_idle() {
                return Err(Failure::new(
                    Code::Busy,
                    "something is playing or queued; retry after the current item",
                ));
            }
            *retiring = true;
            let mut f = self.hello_fields(Vec::new());
            f.insert("takeover".into(), Value::Bool(true));
            return Ok((f, After::Exit));
        }
        if let Some(p) = m.get("protocol").filter(|p| !p.is_null()) {
            let major = p.get("major").and_then(Value::as_u64);
            match major {
                Some(PROTOCOL_MAJOR) => {}
                Some(other) => {
                    return Err(Failure::new(
                        Code::Incompatible,
                        format!(
                            "this host speaks protocol {PROTOCOL_MAJOR}.{PROTOCOL_MINOR}, \
                             not {other}.x"
                        ),
                    ))
                }
                None => return Err(bad("'protocol.major' must be a number")),
            }
        }
        let offered = self.offered();
        let capabilities = self.capabilities();
        let require = str_list(m, "require")?.unwrap_or_default();
        let missing: Vec<&str> = require
            .iter()
            .copied()
            .filter(|r| !capabilities.contains(r) && !offered.contains(r))
            .collect();
        if !missing.is_empty() {
            return Err(Failure::new(
                Code::Unsupported,
                format!("this host does not offer: {}", missing.join(", ")),
            ));
        }
        let wanted = str_list(m, "extensions")?.unwrap_or_default();
        let unavailable: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|e| !offered.contains(e))
            .collect();
        // An extension is enabled for the instance when any client asks for
        // it (or requires it) and stays enabled (spec section 3).
        for e in wanted.iter().chain(require.iter()) {
            if offered.contains(e) {
                self.enable(e)?;
            }
        }
        let keep_alive = opt_bool(m, "keep_alive")?;
        if keep_alive {
            self.lifetime.set_keep_alive();
        }
        // `system` is armed (ducking, hotkeys) while it is needed: for this
        // TCP connection, or for good with keep_alive.
        let wants_system = wanted
            .iter()
            .chain(require.iter())
            .any(|e| *e == system_ext::NAME);
        if let (true, Some(s)) = (wants_system, &self.system) {
            if keep_alive {
                s.keep();
            }
            if session.transport == Transport::Tcp {
                session.system = true;
            }
        }
        session.authed = true;
        Ok((self.hello_fields(unavailable), After::Nothing))
    }

    pub(super) fn speak(&self, m: &Map<String, Value>) -> Handled {
        let text = match m.get("text") {
            Some(Value::String(t)) => t,
            _ => return Err(bad("'text' must be a string")),
        };
        let mode = match opt_str(m, "mode")? {
            None => None,
            Some("append") => Some(QueueMode::Append),
            Some("replace") => Some(QueueMode::Replace),
            Some(other) => return Err(bad(format!("unknown mode '{other}'"))),
        };
        let interrupt = opt_bool(m, "interrupt")?;
        let label = opt_str(m, "label")?.map(str::to_string);
        // Without the extension, `channel` is an unknown field (ignored).
        if let Some(ch) = self.channels.get() {
            if let Some(id) = opt_str(m, "channel")? {
                let _admitted = self.admit()?;
                return channels_ext::speak(ch, id, text, mode, interrupt, label);
            }
        }
        let _admitted = self.admit()?;
        let id = self
            .reader
            .speak(text, mode.unwrap_or(QueueMode::Append), interrupt, label)
            .map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert("item_id".into(), json!(id.0));
        Ok((f, After::Nothing))
    }

    /// `control`. A mute holds external engines before it applies and
    /// `mute`/`unmute` leave them held exactly while Sonara is muted
    /// (#227).
    pub(super) fn control(&self, m: &Map<String, Value>) -> Handled {
        let action = opt_str(m, "action")?.ok_or_else(|| bad("missing 'action'"))?;
        if action != "mute" && action != "unmute" {
            return self.control_now(m, action);
        }
        // One mute change at a time; the hold follows it on drop.
        let change = self.quiet.change();
        if action == "mute" {
            change.hold_now();
        }
        self.control_now(m, action)
    }

    pub(super) fn control_now(&self, m: &Map<String, Value>, action: &str) -> Handled {
        if let Some(ch) = self.channels.get() {
            if action == "flush" {
                return self.flush(ch, m);
            }
            let c = match parse_control(action) {
                Some(c) => Some(c),
                None if action == "next_channel" => None,
                None => return Err(bad(format!("unknown action '{action}'"))),
            };
            let _admitted = self.admit()?;
            // With the agent on, a stop without a channel also drops the
            // summaries cooking and the held decisions.
            if let (Some(a), Some(Control::Stop), None) =
                (self.agent.get(), c, opt_str(m, "channel")?)
            {
                a.stop().map_err(agent_ext::failure)?;
                return Ok((Map::new(), After::Nothing));
            }
            return channels_ext::control(ch, c, m);
        }
        let c = match parse_control(action) {
            Some(c) => c,
            None if EXTENSION_ACTIONS.contains(&action) => {
                return Err(Failure::new(
                    Code::Unsupported,
                    format!("action '{action}' belongs to an extension"),
                ))
            }
            None => return Err(bad(format!("unknown action '{action}'"))),
        };
        let _admitted = self.admit()?;
        self.reader.control(c).map_err(reader_failure)?;
        Ok((Map::new(), After::Nothing))
    }

    pub(super) fn key(&self, m: &Map<String, Value>) -> Result<Key, Failure> {
        let name = opt_str(m, "key")?.ok_or_else(|| bad("missing 'key'"))?;
        match Key::parse(name) {
            Some(k) => Ok(k),
            None if EXTENSION_KEYS.contains(&name) => Err(Failure::new(
                Code::Unsupported,
                format!("setting '{name}' belongs to an extension"),
            )),
            None => Err(bad(format!("unknown setting '{name}'"))),
        }
    }

    pub(super) fn key_value(&self, key: Key) -> Handled {
        let value = self.reader.get(key).map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert("key".into(), json!(key.as_str()));
        f.insert("value".into(), wire::setting_to_json(&value));
        Ok((f, After::Nothing))
    }

    /// `get runtime` also carries `engine_status` (as in `state`, #214), so
    /// a client that polls instead of subscribing sees the engine's
    /// readiness and its model download.
    pub(super) fn add_engine_status(&self, handled: &mut Handled) {
        let Ok((fields, _)) = handled else {
            return;
        };
        let Ok(status) = self.reader.engine_status() else {
            return;
        };
        let engine = events::engine_name(&self.engine);
        if let Some(Value::Object(v)) = fields.get_mut("value") {
            v.insert(
                "engine_status".into(),
                wire::engine_status_json(&engine, &status),
            );
        }
    }

    /// `set`, persisted once it took effect: the value now in force is
    /// stored for every key of the schema (`summaries` is stored by the
    /// agent extension, field by field; the keymap and channel preferences
    /// have files of their own).
    pub(super) fn set(&self, m: &Map<String, Value>) -> Handled {
        let _one_at_a_time = self.setting.lock().unwrap_or_else(|p| p.into_inner());
        let engine_before = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mute_before = self.agent.get().map(|a| a.settings().mute_level);
        let mute_key = opt_str(m, "key").ok().flatten() == Some("mute_level");
        // One mute change at a time; the hold follows it on drop.
        let change = mute_key.then(|| self.quiet.change());
        if let Some(c) = &change {
            if m.get("value")
                .and_then(Value::as_u64)
                .is_some_and(|l| l >= 1)
            {
                // Nothing is sent from here on (#227).
                c.hold_now();
            }
        }
        let result = self.set_now(m);
        // The hold follows the outcome before the cue is spoken.
        drop(change);
        if let (Ok((fields, _)), Some(name)) = (&result, opt_str(m, "key").ok().flatten()) {
            // The mute level's spoken cue (the hotkey's, #197), when it
            // changed; `audio_mode` and `duck_level` speak theirs in the
            // system extension.
            if let (Some(s), "mute_level", Some(before)) = (&self.system, name, mute_before) {
                let now = fields.get("value").and_then(Value::as_u64);
                if let Some(level) = now.filter(|l| *l != u64::from(before)) {
                    s.cue_local(crate::cues::mute_level_cue(level));
                }
            }
            if name != "summaries" && config::setting(name).is_some() {
                if let Some(v) = fields.get("value") {
                    self.store.record(name, v);
                }
                if name == "engine" {
                    self.engine_switched(&engine_before);
                }
            }
        }
        result
    }

    /// `set`/`get` of `debug_log` (#219), a host setting: text and
    /// payloads in the troubleshooting log, on or off.
    pub(super) fn debug_setting(&self, m: &Map<String, Value>, set: bool) -> Option<Handled> {
        if opt_str(m, "key").ok().flatten() != Some(trace_log::DEBUG_KEY) {
            return None;
        }
        if set {
            let raw = m.get("value").unwrap_or(&Value::Null);
            match config::validate(trace_log::DEBUG_KEY, raw) {
                Ok(v) => trace_log::set_debug(v.as_bool().unwrap_or(true)),
                Err(e) => return Some(Err(bad(e))),
            }
        }
        let mut f = Map::new();
        f.insert("key".into(), json!(trace_log::DEBUG_KEY));
        f.insert("value".into(), json!(trace_log::debug()));
        Some(Ok((f, After::Nothing)))
    }

    pub(super) fn set_now(&self, m: &Map<String, Value>) -> Handled {
        if let Some(done) = self.debug_setting(m, true) {
            return done;
        }
        if let Some(done) = self.extension_setting(m, true) {
            return done;
        }
        let key = self.key(m)?;
        let raw = m.get("value").unwrap_or(&Value::Null);
        let value = wire::setting_from_json(raw)
            .ok_or_else(|| bad("'value' must be a non-negative integer, a string or null"))?;
        self.reader.set(key, value).map_err(reader_failure)?;
        if key == Key::Engine {
            if let Ok(sonara_reader::Value::Text(t)) = self.reader.get(Key::Engine) {
                *self.engine.lock().unwrap_or_else(|p| p.into_inner()) = t;
            }
        }
        self.key_value(key)
    }

    pub(super) fn get(&self, m: &Map<String, Value>) -> Handled {
        if let Some(done) = self.debug_setting(m, false) {
            return done;
        }
        if let Some(done) = self.extension_setting(m, false) {
            return done;
        }
        let key = self.key(m)?;
        self.key_value(key)
    }

    pub(super) fn voices(&self, m: &Map<String, Value>) -> Handled {
        if m.contains_key("profile") {
            let engines = self.engines.as_ref().ok_or_else(engines_ext::refused)?;
            return engines_ext::draft_voices(engines, m);
        }
        let engine = opt_str(m, "engine")?;
        let refresh = opt_bool(m, "refresh")?;
        if let (Some(e), Some(id)) = (&self.engines, engine) {
            if let Some(done) = engines_ext::voices(e, &self.reader, id, refresh) {
                return done;
            }
        }
        let voices = self.reader.voices(engine).map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert(
            "voices".into(),
            Value::Array(voices.iter().map(wire::voice_json).collect()),
        );
        Ok((f, After::Nothing))
    }

    pub(super) fn subscribe(&self, session: &Session, m: &Map<String, Value>) -> Handled {
        if session.transport == Transport::Http {
            return Err(bad("over HTTP, subscribe with GET /v1/events?events=..."));
        }
        let set = match str_list(m, "events")? {
            None => EventSet::ALL,
            Some(names) => EventSet::parse(names).map_err(|n| {
                Failure::new(Code::Unsupported, format!("unknown event stream '{n}'"))
            })?,
        };
        if set.earcons && self.agent.get().is_none() {
            return Err(Failure::new(
                Code::Unsupported,
                "the 'earcons' events belong to the agent extension",
            ));
        }
        if set.cues && !self.system.as_ref().is_some_and(|s| s.is_enabled()) {
            return Err(Failure::new(
                Code::Unsupported,
                "the 'cues' events belong to the system extension",
            ));
        }
        let rx = self.events(set)?;
        let mut f = Map::new();
        f.insert("events".into(), json!(set.names()));
        Ok((f, After::Subscribe(rx)))
    }

    /// `shutdown` (extension `system`, #202): the user's stop (`sonara
    /// stop`, uninstall, an upgrade replacing this runtime). Whatever is
    /// reading or queued ends; requests after it are `E_BUSY`, and the
    /// process exits once the reply went out, restoring other apps first.
    pub(super) fn shutdown(&self) -> Handled {
        *self.retiring.lock().unwrap_or_else(|p| p.into_inner()) = true;
        let _ = self.reader.control(Control::Stop);
        Ok((Map::new(), After::Exit))
    }
}
