//! `engines.json`: reading, migrating and saving the file.
use super::*;

/// The text of `engines.json` (`None` when it is missing).
pub(super) fn read_text(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok()
}

/// The `engines` array of `engines.json` (none when the file is missing)
/// and whether the file is of an older format (to migrate), or why the
/// file cannot be read.
pub(super) fn parse_file(text: Option<&str>) -> Result<(Vec<Value>, bool), String> {
    match text {
        None => Ok((Vec::new(), false)),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(v) => Ok((
                v.get("engines")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                v.get("format").and_then(Value::as_u64).unwrap_or(1) < FORMAT,
            )),
            Err(e) => Err(format!("{FILE} is not valid JSON ({e})")),
        },
    }
}

/// Bind what a format 1 file left unbound to the entry's origin now (the
/// file is local, so its addresses are the user's): Credential Manager keys
/// without an origin, and `env:` entries without `key_origin`. Problems are
/// for the log; a key that cannot be bound stays unbound and is not sent.
pub(super) fn migrate(store: &dyn KeyStore, raws: &mut [Value], problems: &mut Vec<String>) {
    for raw in raws.iter_mut() {
        let Some(id) = raw.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let origin = origin_of(raw);
        let env = raw
            .get("key_ref")
            .and_then(Value::as_str)
            .is_some_and(|k| k.starts_with("env:"));
        if let (true, Some(o), Some(m)) = (env, &origin, raw.as_object_mut()) {
            m.entry(KEY_ORIGIN).or_insert_with(|| json!(o));
        }
        if let Ok(Some(k)) = store.get(&id) {
            if k.origin.is_none() {
                let done = match &origin {
                    Some(o) => store.set(&id, &k.secret, o).map_err(|e| e.to_string()),
                    None => Err("the engine has no address".into()),
                };
                if let Err(e) = done {
                    problems.push(format!(
                        "engine '{id}': its key could not be bound to an address ({e}); \
                         enter it again"
                    ));
                }
            }
        }
    }
}

impl Engines {
    /// The entries of the file's profiles. An entry of `old` whose stored
    /// profile is the same is kept as it is (the same engine).
    pub(super) fn entries_from(
        &self,
        raws: Vec<Value>,
        mut old: Vec<Entry>,
        problems: &mut Vec<String>,
    ) -> Vec<Entry> {
        let mut entries = Vec::new();
        for raw in raws.into_iter().take(MAX_PROFILES) {
            let id = raw
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() || entries.iter().any(|e: &Entry| e.id == id) {
                problems.push(format!("{FILE}: an entry without a unique id was skipped"));
                continue;
            }
            if let Some(i) = old.iter().position(|e| e.id == id && e.raw == raw) {
                let kept = old.swap_remove(i);
                if let State::Invalid(m) = &kept.state {
                    problems.push(format!("{FILE}: engine '{id}' is not used: {m}"));
                }
                entries.push(kept);
                continue;
            }
            let state = match self.build(&raw) {
                Ok(e) => State::Ready(e),
                Err(ProfileError::Unsupported { kind, .. }) => {
                    problems.push(format!(
                        "{FILE}: engine '{id}' of kind '{kind}' is not supported by this \
                         version; kept, not used"
                    ));
                    State::Unsupported
                }
                Err(ProfileError::Invalid(m)) => {
                    problems.push(format!("{FILE}: engine '{id}' is not used: {m}"));
                    State::Invalid(m)
                }
            };
            entries.push(Entry { id, raw, state });
        }
        entries
    }

    /// `engine_reload`: read `engines.json` again, the only way a `command`
    /// profile reaches a running runtime. It takes nothing from the
    /// request. An unchanged profile keeps its engine; a changed one is
    /// replaced in the registries, a gone or unusable one unregistered.
    /// A file that is not JSON changes nothing.
    pub fn reload(&self) -> Result<Reloaded, Failure> {
        let mut out = Reloaded::default();
        let mut entries = self.lock();
        let text = read_text(&self.file);
        let (raws, _) =
            parse_file(text.as_deref()).map_err(|why| bad(format!("{why}; nothing changed")))?;
        // An edit of the file is an edit path too (spec 6.4): a key whose
        // origin is no longer its entry's is deleted. A reload never binds
        // a key (only the migration at start does).
        for raw in &raws {
            self.drop_key_bound_elsewhere(raw);
        }
        *self.seen() = text;
        let old: Vec<Entry> = entries.drain(..).collect();
        let before: Vec<(String, Arc<External>)> = old
            .iter()
            .filter_map(|e| match &e.state {
                State::Ready(x) => Some((e.id.clone(), x.clone())),
                _ => None,
            })
            .collect();
        let next = self.entries_from(raws, old, &mut out.problems);
        *entries = next;
        let now: Vec<(String, Arc<External>)> = entries
            .iter()
            .filter_map(|e| match &e.state {
                State::Ready(x) => Some((e.id.clone(), x.clone())),
                _ => None,
            })
            .collect();
        drop(entries);
        let registries = self.registries();
        for (id, x) in &before {
            match now.iter().find(|(i, _)| i == id) {
                Some((_, y)) if Arc::ptr_eq(x, y) => {}
                Some((_, y)) => {
                    x.cancel();
                    for r in &registries {
                        let _ = r.replace(y.clone());
                    }
                    out.changed.push(id.clone());
                }
                None => {
                    x.cancel();
                    for r in &registries {
                        let _ = r.unregister(id);
                    }
                    out.changed.push(id.clone());
                }
            }
        }
        for (id, y) in &now {
            if !before.iter().any(|(i, _)| i == id) {
                for r in &registries {
                    if let Err(e) = r.register(y.clone()) {
                        self.note(&format!("engine {id}: not registered: {e}"));
                    }
                }
            }
        }
        for p in &out.problems {
            self.note(p);
        }
        self.note(&format!(
            "engine reload engines={} changed={}",
            self.lock().len(),
            out.changed.len()
        ));
        Ok(out)
    }

    pub(super) fn seen(&self) -> MutexGuard<'_, Option<String>> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn take_in_file_changes(&self) -> Result<(), Failure> {
        let changed = *self.seen() != read_text(&self.file);
        if changed {
            self.note(&format!(
                "{FILE} changed on disk; read again before the save"
            ));
            self.reload()?;
        }
        Ok(())
    }

    pub(super) fn save(&self, entries: &[Entry]) -> Result<(), Failure> {
        let v = json!({
            "format": FORMAT,
            "engines": entries.iter().map(|e| e.raw.clone()).collect::<Vec<_>>(),
        });
        crate::config::write_json(&self.file, &v)
            .map_err(|e| Failure::new(Code::Engine, format!("cannot save {FILE}: {e}")))?;
        *self.seen() = read_text(&self.file);
        Ok(())
    }
}
