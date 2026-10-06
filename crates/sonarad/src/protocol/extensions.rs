//! The extensions (spec 4.2 to 4.4): enabling them, and their keys of
//! `set`/`get` and actions of `control`.
use super::*;

impl Server {
    /// The `channels` extension, if a client enabled it.
    pub fn channels(&self) -> Option<&Channels> {
        self.channels.get()
    }

    /// The `agent` extension, if a client enabled it.
    pub fn agent(&self) -> Option<&Agent> {
        self.agent.get()
    }

    /// Enable an extension this host offers (idempotent). `agent` needs
    /// `channels` and enables it first. The layer starts with the persisted
    /// settings.
    pub(super) fn enable(&self, name: &str) -> Result<(), Failure> {
        let _one = self.enabling.lock().unwrap_or_else(|p| p.into_inner());
        let engine = |e: String| Failure::new(Code::Engine, e);
        if (name == channels_ext::NAME || name == agent_ext::NAME) && self.channels.get().is_none()
        {
            let config = sonara_channels::Config {
                announce: self.store.value(channels_ext::ANNOUNCE_KEY) != "off",
                ..Default::default()
            };
            let ch =
                Channels::new(self.reader.clone(), config).map_err(|e| engine(e.to_string()))?;
            // The channels the user muted stay muted (#196).
            channels_ext::apply_mutes(&ch, &self.store);
            if let Some(log) = self.log.clone() {
                let origins = self.origins.clone();
                ch.on_drop(Some(Arc::new(move |d: &sonara_channels::Dropped| {
                    let origin = origins.get(d.entry);
                    log(&trace_log::drop_line(
                        d,
                        origin.as_ref(),
                        trace_log::debug(),
                    ));
                })));
            }
            let _ = self.channels.set(ch);
        }
        if name == agent_ext::NAME && self.agent.get().is_none() {
            let ch = self.channels.get().expect("enabled above").clone();
            let config = sonara_agent::Config {
                settings: agent_ext::settings_from(&self.store),
                earcons: self.earcons.clone(),
                ..Default::default()
            };
            let agent = match Agent::new(ch.clone(), config) {
                Ok(a) => a,
                // A runtime without the summarizer: start with summaries off.
                Err(sonara_agent::Error::NoSummarizer) => {
                    let mut settings = agent_ext::settings_from(&self.store);
                    settings.summaries.enabled = false;
                    let config = sonara_agent::Config {
                        settings,
                        earcons: self.earcons.clone(),
                        ..Default::default()
                    };
                    Agent::new(ch, config).map_err(|e| engine(e.to_string()))?
                }
                Err(e) => return Err(engine(e.to_string())),
            };
            if let Some(log) = self.log.clone() {
                let origins = self.origins.clone();
                agent.on_trace(Some(Arc::new(move |t: &sonara_agent::Trace| {
                    match &t.what {
                        sonara_agent::Traced::Spoken { kind, entry, .. }
                        | sonara_agent::Traced::Stored { kind, entry, .. } => {
                            origins.record(*entry, kind, &t.source);
                        }
                        _ => {}
                    }
                    log(&trace_log::agent_line(t, trace_log::debug()));
                })));
            }
            let _ = self.agent.set(agent);
            // A saved mute level applies from the start (#227).
            self.quiet.sync();
        }
        if name == system_ext::NAME {
            if let Some(s) = &self.system {
                s.enable(&self.reader)?;
            }
        }
        Ok(())
    }

    /// The extensions enabled now.
    pub fn enabled(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.channels.get().is_some() {
            v.push(channels_ext::NAME);
        }
        if self.agent.get().is_some() {
            v.push(agent_ext::NAME);
        }
        if self.system.as_ref().is_some_and(|s| s.is_enabled()) {
            v.push(system_ext::NAME);
        }
        v
    }

    /// `control flush` (#228): the flush hotkey. It stops the session
    /// being read; with the agent the rest of that reply is skipped, and
    /// with `flush_scope` `all` every other session's ready messages go
    /// too.
    pub(super) fn flush(&self, ch: &Channels, m: &Map<String, Value>) -> Handled {
        if opt_str(m, "channel")?.is_some() {
            return Err(bad(
                "'flush' acts on the session being read; use 'stop' with a channel",
            ));
        }
        let _admitted = self.admit()?;
        let (report, scope) = match self.agent.get() {
            Some(a) => (
                a.flush().map_err(agent_ext::failure)?,
                a.settings().flush_scope,
            ),
            None => (
                sonara_channels::FlushReport {
                    flushed: ch.stop_reading("flush").map_err(channels_ext::failure)?,
                    others: Vec::new(),
                },
                sonara_agent::FlushScope::Session,
            ),
        };
        Ok((channels_ext::flushed_fields(&report, scope), After::Nothing))
    }

    /// `set`/`get` of an enabled extension's key, if `m` names one.
    pub(super) fn extension_setting(&self, m: &Map<String, Value>, set: bool) -> Option<Handled> {
        let name = opt_str(m, "key").ok().flatten()?;
        let value = if set {
            Some(m.get("value").unwrap_or(&Value::Null))
        } else {
            None
        };
        if let (Some(a), true) = (self.agent.get(), agent_ext::KEYS.contains(&name)) {
            return Some(agent_ext::setting(a, &self.store, name, value));
        }
        if let Some(s) = self.system.as_ref().filter(|s| s.is_enabled()) {
            if system_ext::KEYS.contains(&name) {
                let mut handled = s.setting(name, value);
                if name == "runtime" {
                    self.add_engine_status(&mut handled);
                }
                return Some(handled);
            }
        }
        let ch = self.channels.get()?;
        match name {
            channels_ext::ANNOUNCE_KEY => Some(channels_ext::announce(ch, value)),
            channels_ext::PREFS_KEY => Some(channels_ext::prefs_setting(
                ch,
                self.agent.get(),
                &self.store,
                value,
            )),
            _ => None,
        }
    }
}
