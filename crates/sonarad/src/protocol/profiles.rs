//! The `engine_*` messages of external engine profiles (spec 10.2), and
//! what follows a switch of the engine.
use super::*;

impl Server {
    /// `engine_list`, `engine_add`, `engine_remove`, `engine_key`,
    /// `engine_test` (spec 10.2), `engine_reload` (protocol 1.3).
    pub(super) fn engine_message(&self, kind: &str, m: &Map<String, Value>) -> Handled {
        let engines = self.engines.as_ref().ok_or_else(engines_ext::refused)?;
        let current = self.current_engine();
        match kind {
            "engine_list" => engines_ext::list(engines, &self.reader, &current),
            "engine_add" => engines_ext::add(engines, &self.reader, m, &current),
            "engine_key" => engines_ext::key(engines, m),
            // The provider round trip runs outside the admission lock
            // (it can take the profile's whole timeout); only the play is
            // admitted, so speech and controls never wait for a test.
            "engine_test" => engines_ext::test(engines, &self.reader, m, &current, || self.admit()),
            "engine_models" => engines_ext::models(engines, m),
            "engine_reload" => self.engine_reload(engines, &current),
            _ => self.engine_remove(engines, m, &current),
        }
    }

    /// `engine_reload` (protocol 1.3, no fields): read `engines.json` again
    /// after the user (or `sonara engines add --kind command`) changed it.
    /// It takes no profile: a `command` engine reaches the runtime only
    /// through the file. A current engine that is gone or unusable now is
    /// switched to the default choice; a changed one applies to the next
    /// chunk. Replies like `engine_list`, plus `problems`.
    pub(super) fn engine_reload(&self, engines: &Engines, current: &str) -> Handled {
        let done = engines.reload()?;
        if done.changed.iter().any(|id| id == current) {
            let value = if engines.get(current).is_some() {
                current
            } else {
                engines.default_engine()
            };
            let mut set = Map::new();
            set.insert("key".into(), json!("engine"));
            set.insert("value".into(), json!(value));
            self.set(&set)?;
        }
        let mut f = engines.list(&self.current_engine());
        f.insert("problems".into(), json!(done.problems));
        Ok((f, After::Nothing))
    }

    /// `engine_remove` `{engine, forget_key?}`: a current engine is switched
    /// to the default choice first (saved like any `set`).
    pub(super) fn engine_remove(
        &self,
        engines: &Engines,
        m: &Map<String, Value>,
        current: &str,
    ) -> Handled {
        let id = opt_str(m, "engine")?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("missing 'engine'"))?;
        let forget_key = match m.get("forget_key") {
            None | Some(Value::Null) => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad("'forget_key' must be true or false")),
        };
        if !engines.contains(id) {
            return Err(Failure::new(Code::NotFound, format!("no engine '{id}'")));
        }
        if current == id {
            let mut set = Map::new();
            set.insert("key".into(), json!("engine"));
            set.insert("value".into(), json!(engines.default_engine()));
            self.set(&set)?;
        }
        engines.remove(id, forget_key)?;
        let mut f = Map::new();
        f.insert("removed".into(), json!(id));
        f.insert("engine".into(), json!(self.current_engine()));
        Ok((f, After::Nothing))
    }

    /// After `set engine`: a saved voice the new engine lacks is replaced by
    /// the voice the reader now uses. Picking the same engine again keeps a
    /// saved voice that could not apply (it waits until it is available).
    pub(super) fn engine_switched(&self, before: &str) {
        let now = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if now == before {
            return;
        }
        let Some(Value::String(saved)) = self.store.user("voice") else {
            // No voice of the user's: the default voice (af_sarah) comes
            // back with an engine that lists it (an external engine takes
            // any id, so it must list it: its own voice applies otherwise).
            if let Some(Value::String(d)) = config::default("voice") {
                let listed = self
                    .reader
                    .voices(Some(&now))
                    .is_ok_and(|vs| vs.iter().any(|v| v.id == d));
                if listed {
                    let _ = self.reader.set(Key::Voice, sonara_reader::Value::Text(d));
                }
            }
            return;
        };
        let offered = self
            .reader
            .voices(Some(&now))
            .map(|vs| vs.iter().any(|v| v.id == saved || v.name == saved))
            .unwrap_or(false);
        if !offered {
            if let Ok(v) = self.reader.get(Key::Voice) {
                self.store.record("voice", &wire::setting_to_json(&v));
            }
        }
    }
}
