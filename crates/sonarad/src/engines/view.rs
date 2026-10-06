//! How a profile is shown in `engine_list` and `engine_get`.
use super::*;

impl Engines {
    /// The profile view of spec 10.1 (never a secret).
    pub(super) fn view(&self, e: &Entry, current: &str, status: Option<Value>) -> Value {
        let mut m = match &e.state {
            State::Ready(x) => {
                let p = x.profile();
                let mut m = p.to_json().as_object().cloned().unwrap_or_default();
                // What the profile itself sets, so an edit keeps the defaults
                // as defaults (an Azure region change still moves the URL).
                let explicit: Map<String, Value> = ["url", "model", "voice", "send_mode"]
                    .into_iter()
                    .filter_map(|k| m.get(k).map(|v| (k.to_string(), v.clone())))
                    .collect();
                m.insert("explicit".into(), Value::Object(explicit));
                // The address in force (a preset's or kind's default
                // included). No model or voice is ever filled in (#235).
                if let Some(u) = p.base_url() {
                    m.insert("url".into(), json!(u));
                }
                // What the form needs to know about the model (#235).
                m.insert("takes_model".into(), json!(p.takes_model()));
                m.insert("model_required".into(), json!(p.model_required()));
                m.insert("model_list".into(), json!(x.has_model_list()));
                // What the user must still pick: "model", "voice" (the
                // reader's voice may supply the voice: `engines_ext::list`).
                let mut missing = Vec::new();
                if p.missing_model() {
                    missing.push("model");
                }
                if p.voice_required() && p.voice.is_none() {
                    missing.push("voice");
                }
                m.insert("missing".into(), json!(missing));
                m.insert("label".into(), json!(p.display_label()));
                m.insert("key_present".into(), json!(x.key_present()));
                m.insert("sends_text_to".into(), json!(p.sends_text_to()));
                m.insert("local".into(), json!(p.is_local()));
                // "Send to the engine" in force (#235): the profile's, else
                // the kind's default (`explicit` says which).
                m.insert("send_mode".into(), json!(p.send_mode().as_str()));
                m.insert("supported".into(), json!(true));
                m
            }
            State::Unsupported | State::Invalid(_) => {
                let mut m = Map::new();
                for k in [
                    "id",
                    "kind",
                    "label",
                    "url",
                    "model",
                    "voice",
                    "key_ref",
                    "send_mode",
                    "options",
                ] {
                    if let Some(v) = e.raw.get(k) {
                        m.insert(k.into(), v.clone());
                    }
                }
                m.insert("key_present".into(), json!(false));
                let host = e
                    .raw
                    .get("url")
                    .and_then(Value::as_str)
                    .and_then(|u| sonara_engine::external::profile::Url::parse(u).ok());
                m.insert(
                    "sends_text_to".into(),
                    json!(host.as_ref().map(|u| u.host.clone()).unwrap_or_default()),
                );
                m.insert("local".into(), json!(host.is_some_and(|u| u.is_loopback())));
                m.insert(
                    "supported".into(),
                    json!(matches!(e.state, State::Invalid(_))),
                );
                if let State::Invalid(why) = &e.state {
                    m.insert("error".into(), json!(why));
                }
                m
            }
        };
        m.insert("license_class".into(), json!("external"));
        m.insert("current".into(), json!(e.id == current));
        m.insert("status".into(), status.unwrap_or(Value::Null));
        Value::Object(m)
    }

    pub(super) fn status_of(e: &Entry) -> Option<Value> {
        match &e.state {
            State::Ready(x) => {
                let mut s = wire::engine_status_json(&e.id, &x.status());
                s.as_object_mut().map(|o| o.remove("engine"));
                Some(s)
            }
            _ => None,
        }
    }

    /// `engine_list`.
    pub fn list(&self, current: &str) -> Map<String, Value> {
        let entries = self.lock();
        let views: Vec<Value> = entries
            .iter()
            .map(|e| self.view(e, current, Self::status_of(e)))
            .collect();
        let mut f = Map::new();
        f.insert("engines".into(), Value::Array(views));
        let builtin: Vec<&str> = ["kokoro", "onecore", "fake"]
            .into_iter()
            .filter(|b| self.registries().first().is_some_and(|r| r.get(b).is_ok()))
            .collect();
        f.insert("builtin".into(), json!(builtin));
        f.insert(
            "kinds".into(),
            json!(implemented_kinds()
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()),
        );
        f.insert(
            "presets".into(),
            json!(Preset::ALL.iter().map(Preset::as_str).collect::<Vec<_>>()),
        );
        f
    }

    /// The view of one profile.
    pub fn view_of(&self, id: &str, current: &str) -> Option<Value> {
        let entries = self.lock();
        let e = entries.iter().find(|e| e.id == id)?;
        Some(self.view(e, current, Self::status_of(e)))
    }
}
