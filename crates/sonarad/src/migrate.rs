//! Migration from the Python plugin (#201): the first runtime on a home
//! with no `config.json` imports the user's settings from the plugin's
//! folder (`%USERPROFILE%\.sonara`):
//!
//! - `config.json`: voice, rate, volume, audio mode, duck level, mute
//!   level, verbosity, minimum queue and the summary settings (mode,
//!   command, model, timeout, settle time, style, custom prompts). Values
//!   are validated by the runtime's schema; a value equal to the runtime's
//!   default is not stored (it was not a choice). Mapped like the plugin's
//!   own loader: a removed Chatterbox voice speaks as `af_heart`, the
//!   pre-#92 `audio_control: true` is `audio_mode: duck`, a speech gain
//!   above 100 % is 100, and in a file from before the plugin's format 2
//!   the old defaults `duck_level: 20` and `summary_timeout: 20` count as
//!   unset. The cue voice and fast cues are not carried over: the runtime
//!   speaks its control cues in the voice in force. `background_policy`
//!   `earcon_only` stays the default; any other value is `all`.
//! - `keymap.json`: `nav_start` is `restart`, `next_session` is
//!   `next_channel`; a binding with a key or modifier the runtime lacks is
//!   left out (and noted). Only when the home has no `keymap.json` yet.
//! - `session_prefs.json`: per-session `name` (as `label`), `voice` and
//!   `muted`. Only when the home has none yet.
//! - The user's own earcons (`config.json` `earcons`: `{kind: path}`):
//!   each file that exists is copied to `<home>\earcons\<kind>.wav` (the
//!   runtime's custom earcons folder), unless one is there already. Paths
//!   to the plugin's bundled WAVs (older versions saved them) are skipped.
//!
//! The plugin's folder is only read, never changed. `config.json` gets the
//! marker key `_migrated` (written even when nothing was imported), so the
//! migration runs once: a home with a `config.json` is never migrated.
use crate::config::{self, Prefs};
use serde_json::{json, Map, Value};
use sonara_system::keymap::{self, Action, Binding};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Kokoro's voices in the Python plugin (`kokoro.VOICES`): a saved voice of
/// these is never a Chatterbox one.
const KOKORO_VOICES: &[&str] = &[
    "af_heart",
    "af_bella",
    "bf_emma",
    "af_nicole",
    "af_aoede",
    "af_kore",
    "af_sarah",
    "am_fenrir",
    "am_michael",
    "am_puck",
    "af_alloy",
    "af_nova",
    "bf_isabella",
    "bm_fable",
    "bm_george",
    "af_sky",
    "bm_lewis",
    "af_jessica",
    "af_river",
    "am_echo",
    "am_eric",
    "am_liam",
    "am_onyx",
    "bf_alice",
    "bf_lily",
    "bm_daniel",
    "am_santa",
    "am_adam",
];

/// What a removed Chatterbox voice speaks as (the plugin's #134 rule).
pub const CHATTERBOX_REPLACEMENT: &str = "af_heart";

/// The plugin's folder: `%USERPROFILE%\.sonara` (Python's `Path.home()`).
pub fn default_legacy_dir() -> Option<PathBuf> {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .map(|h| Path::new(&h).join(".sonara"))
}

fn read_object(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The voice the runtime should use for a voice saved by the plugin: the
/// `kokoro:` prefix dropped, a Chatterbox voice mapped to `af_heart`.
pub fn map_voice(name: &str, legacy_dir: &Path) -> Option<String> {
    let s = name.trim();
    if s.is_empty() {
        return None;
    }
    let (engine, rest) = match s.split_once(':') {
        Some((e, r)) => (Some(e.trim().to_ascii_lowercase()), r.trim()),
        None => (None, s),
    };
    if engine.as_deref() == Some("chatterbox") {
        return Some(CHATTERBOX_REPLACEMENT.into());
    }
    if engine.as_deref() == Some("kokoro") {
        return Some(rest.to_ascii_lowercase());
    }
    if KOKORO_VOICES.contains(&s.to_ascii_lowercase().as_str()) {
        return Some(s.to_ascii_lowercase());
    }
    let lower = s.to_ascii_lowercase();
    let clips = legacy_dir.join("voices").join("chatterbox");
    let is_clip = std::fs::read_dir(&clips)
        .map(|it| {
            it.filter_map(Result::ok).any(|e| {
                let p = e.path();
                p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav"))
                    && p.file_stem()
                        .is_some_and(|st| st.to_string_lossy().to_ascii_lowercase() == lower)
            })
        })
        .unwrap_or(false);
    if lower == "cb_default" || is_clip {
        return Some(CHATTERBOX_REPLACEMENT.into());
    }
    Some(s.to_string())
}

/// A number as an integer (the plugin stored ints, a hand edit may not).
fn int(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64))
}

/// The Python plugin's defaults. A file from before format 2 wrote every
/// key, so there a value equal to these was no choice of the user's and is
/// not imported (the runtime's own default applies).
const PYTHON_DEFAULTS: &[(&str, &str)] = &[
    ("rate", "200"),
    ("volume", "100"),
    ("audio_mode", "\"off\""),
    ("duck_level", "20"),
    ("mute_level", "0"),
    ("verbosity", "\"everything\""),
    ("minqueue", "1"),
    ("background_policy", "\"earcon_only\""),
];

fn python_default(key: &str) -> Option<Value> {
    PYTHON_DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| serde_json::from_str(v).ok())
}

/// The runtime's `config.json` keys for the plugin's `config.json`, and
/// notes for the log. A format 2 file holds only the keys the user set, and
/// each is imported, also one equal to the runtime's default (#202: the
/// defaults changed to the maintainer's, so a user who chose the old
/// default keeps it).
pub fn convert_config(
    py: &Map<String, Value>,
    legacy_dir: &Path,
) -> (Map<String, Value>, Vec<String>) {
    let mut out = Map::new();
    let mut notes = Vec::new();
    // Before format 2 the plugin wrote every key; these were its defaults.
    let legacy_file = !py.contains_key("_format");
    let old_default = |key: &str, v: &Value| {
        legacy_file && int(v) == Some(20) && { key == "duck_level" || key == "summary_timeout" }
    };
    let mut put = |key: &str, v: Value, notes: &mut Vec<String>| match config::validate(key, &v) {
        Ok(clean) => {
            if !(legacy_file && python_default(key).as_ref() == Some(&clean)) {
                out.insert(key.to_string(), clean);
            }
        }
        Err(e) => notes.push(format!("not imported: {e} (was {v})")),
    };
    if let Some(v) = py.get("voice").and_then(Value::as_str) {
        if let Some(mapped) = map_voice(v, legacy_dir) {
            if mapped != v {
                notes.push(format!("voice '{v}' is now '{mapped}'"));
            }
            put("voice", json!(mapped), &mut notes);
        }
    }
    if let Some(r) = py.get("rate").and_then(int) {
        put("rate", json!(r.clamp(100, 400)), &mut notes);
    }
    if let Some(v) = py.get("volume").and_then(int) {
        if v > 100 {
            notes.push(format!(
                "speech volume {v} % is 100 % (the runtime does not amplify)"
            ));
        }
        put("volume", json!(v.clamp(0, 100)), &mut notes);
    }
    let mode = py
        .get("audio_mode")
        .cloned()
        .or_else(|| (py.get("audio_control") == Some(&Value::Bool(true))).then(|| json!("duck")));
    if let Some(m) = mode {
        put("audio_mode", m, &mut notes);
    }
    if let Some(v) = py
        .get("duck_level")
        .filter(|v| !old_default("duck_level", v))
    {
        if let Some(n) = int(v) {
            put("duck_level", json!(n.clamp(0, 100)), &mut notes);
        }
    }
    for key in ["mute_level", "minqueue"] {
        if let Some(n) = py.get(key).and_then(int) {
            let (lo, hi) = if key == "mute_level" { (0, 2) } else { (0, 10) };
            put(key, json!(n.clamp(lo, hi)), &mut notes);
        }
    }
    if let Some(v) = py.get("verbosity") {
        put("verbosity", v.clone(), &mut notes);
    }
    // sessions.py: "earcon_only" holds background sessions back; any other
    // value reads every session.
    if let Some(v) = py.get("background_policy") {
        let policy = if v.as_str() == Some("earcon_only") {
            "earcon_only"
        } else {
            "all"
        };
        put("background_policy", json!(policy), &mut notes);
    }

    // Summaries: one object in the runtime.
    let mut s = Map::new();
    if let Some(b) = py.get("summary_mode").and_then(Value::as_bool) {
        s.insert("enabled".into(), json!(b));
    }
    for (from, to) in [
        ("summary_command", "command"),
        ("summary_model", "model"),
        ("summary_style", "style"),
    ] {
        if let Some(v) = py.get(from) {
            s.insert(to.into(), v.clone());
        }
    }
    if let Some(v) = py
        .get("summary_timeout")
        .filter(|v| !old_default("summary_timeout", v))
    {
        if let Some(n) = int(v) {
            s.insert("timeout".into(), json!(n.clamp(15, 300)));
        }
    }
    if let Some(n) = py.get("summary_settle_ms").and_then(int) {
        s.insert("settle_ms".into(), json!(n.clamp(0, 5000)));
    }
    if let Some(Value::Object(p)) = py.get("summary_prompts") {
        let kept: Map<String, Value> = p
            .iter()
            .filter(|(k, v)| config::STYLES.contains(&k.as_str()) && v.as_str().is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        s.insert("prompts".into(), Value::Object(kept));
    }
    let defaults = config::default("summaries").unwrap_or_default();
    let mut summaries = Map::new();
    for (k, v) in s {
        match config::validate_summaries(&json!({ k.clone(): v.clone() })) {
            Ok(clean) => {
                for (ck, cv) in clean {
                    let empty_prompts =
                        ck == "prompts" && cv.as_object().is_some_and(Map::is_empty);
                    if defaults.get(&ck) != Some(&cv) && !empty_prompts {
                        summaries.insert(ck, cv);
                    }
                }
            }
            Err(e) => notes.push(format!("not imported: {e} (was {v})")),
        }
    }
    if !summaries.is_empty() {
        out.insert("summaries".into(), Value::Object(summaries));
    }
    if py.contains_key("cue_voice") || py.contains_key("fast_cues") {
        notes.push(
            "cue voice and fast cues are not imported: the runtime speaks its control cues in the voice in force"
                .into(),
        );
    }
    (out, notes)
}

/// The runtime's keymap overrides for the plugin's `keymap.json`.
pub fn convert_keymap(py: &Map<String, Value>) -> (BTreeMap<String, Binding>, Vec<String>) {
    let mut out: BTreeMap<String, Binding> = BTreeMap::new();
    let mut notes = Vec::new();
    for (name, v) in py {
        let Some(action) = Action::parse(name) else {
            notes.push(format!(
                "hotkey '{name}' is not an action of the runtime; skipped"
            ));
            continue;
        };
        let key = v
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let mods: Vec<String> = v
            .get("mods")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|m| m.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let binding = if key.is_empty() {
            Binding::unbound()
        } else if keymap::key_code(key).is_none() {
            notes.push(format!("hotkey {name}: unknown key '{key}'; skipped"));
            continue;
        } else if let Some(m) = mods.iter().find(|m| keymap::mod_mask(m).is_none()) {
            notes.push(format!("hotkey {name}: unknown modifier '{m}'; skipped"));
            continue;
        } else {
            Binding {
                key: Some(key.to_ascii_lowercase()),
                mods,
            }
        };
        // The current name wins over the plugin's alias for one action.
        let current = action.as_str() == name;
        if current || !out.contains_key(action.as_str()) {
            out.insert(action.as_str().to_string(), binding);
        }
    }
    (out, notes)
}

/// The runtime's channel preferences for the plugin's `session_prefs.json`.
pub fn convert_prefs(py: &Map<String, Value>, legacy_dir: &Path) -> Map<String, Value> {
    py.iter()
        .filter(|(id, _)| !id.is_empty())
        .filter_map(|(id, v)| {
            let mut p = Prefs::from_json(v);
            p.voice = p.voice.and_then(|voice| map_voice(&voice, legacy_dir));
            (!p.is_empty()).then(|| (id.clone(), p.to_json()))
        })
        .collect()
}

/// A path to one of the Python plugin's own earcon WAVs (older versions
/// froze them into `config.json`): in its package folder or its app copy.
fn bundled_earcon(path: &str, legacy_dir: &Path) -> bool {
    let p = path.replace('/', "\\").to_lowercase();
    let app = legacy_dir.join("app").to_string_lossy().to_lowercase();
    p.contains(r"sonara\platform\windows\earcons\") || p.starts_with(&app)
}

/// Copy the plugin's custom earcons (`earcons: {kind: path}`) into
/// `<home>\earcons\<kind>.wav`. Returns the notes for the log.
pub fn import_earcons(py: &Map<String, Value>, home: &Path, legacy_dir: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    let Some(Value::Object(map)) = py.get("earcons") else {
        return notes;
    };
    for (kind, path) in map {
        let Some(path) = path.as_str().filter(|p| !p.is_empty()) else {
            continue;
        };
        if bundled_earcon(path, legacy_dir) {
            continue;
        }
        if sonara_agent::Earcon::parse(kind).is_none() {
            notes.push(format!("earcons: unknown kind '{kind}' not imported"));
            continue;
        }
        let src = Path::new(path);
        if !src.is_file() {
            notes.push(format!("earcons: {kind}: {path} is missing; not imported"));
            continue;
        }
        let dir = home.join("earcons");
        let dest = dir.join(format!("{kind}.wav"));
        if dest.exists() {
            continue;
        }
        let copied = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(src, &dest));
        notes.push(match copied {
            Ok(_) => format!("earcons: {kind} imported from {path}"),
            Err(e) => format!("earcons: cannot copy {path} to {}: {e}", dest.display()),
        });
    }
    notes
}

/// Migrate `legacy_dir` into `home` if `home` has no `config.json` and the
/// plugin left files. Returns the notes for the log, or `None` when there
/// was nothing to do.
pub fn run(home: &Path, legacy_dir: &Path) -> Option<Vec<String>> {
    let config_path = home.join(config::CONFIG_FILE);
    if config_path.exists() {
        return None;
    }
    let py_config = read_object(&legacy_dir.join("config.json"));
    let py_keymap = read_object(&legacy_dir.join("keymap.json"));
    let py_prefs = read_object(&legacy_dir.join("session_prefs.json"));
    if py_config.is_none() && py_keymap.is_none() && py_prefs.is_none() {
        return None;
    }
    let mut notes = vec![format!(
        "importing the settings of the Python plugin from {} (left unchanged)",
        legacy_dir.display()
    )];
    let fail = |what: &Path, e: std::io::Error| format!("cannot write {}: {e}", what.display());

    if let Some(km) = py_keymap {
        let path = home.join("keymap.json");
        if path.exists() {
            notes.push(
                "keymap.json exists in the home; the plugin's hotkeys are not imported".into(),
            );
        } else {
            let (bindings, n) = convert_keymap(&km);
            notes.extend(n);
            match sonara_system::state_file::write(&path, &bindings) {
                Ok(()) => notes.push(format!("hotkeys: {} binding(s) imported", bindings.len())),
                Err(e) => notes.push(fail(&path, e)),
            }
        }
    }
    if let Some(prefs) = py_prefs {
        let path = home.join(config::PREFS_FILE);
        if !path.exists() {
            let converted = convert_prefs(&prefs, legacy_dir);
            let count = converted.len();
            match config::write_json(&path, &Value::Object(converted)) {
                Ok(()) => notes.push(format!("session preferences: {count} imported")),
                Err(e) => notes.push(fail(&path, e)),
            }
        }
    }
    if let Some(c) = &py_config {
        notes.extend(import_earcons(c, home, legacy_dir));
    }
    let (mut user, n) = py_config
        .map(|c| convert_config(&c, legacy_dir))
        .unwrap_or_default();
    notes.extend(n);
    let imported: Vec<String> = user.keys().cloned().collect();
    notes.push(if imported.is_empty() {
        "settings: nothing differs from the defaults".to_string()
    } else {
        format!("settings imported: {}", imported.join(", "))
    });
    user.insert(
        config::MARKER.into(),
        json!({"from": legacy_dir.display().to_string(), "version": crate::VERSION}),
    );
    if let Err(e) = config::write_json(&config_path, &Value::Object(user)) {
        notes.push(fail(&config_path, e));
    }
    Some(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonarad-migrate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn the_plugins_own_earcons_are_copied_to_the_earcons_folder() {
        let legacy = tmp();
        let home = tmp();
        let mine = legacy.join("my-chime.wav");
        std::fs::write(&mine, b"RIFF mine").unwrap();
        let bundled = legacy
            .join("app")
            .join("sonara")
            .join("platform")
            .join("windows")
            .join("earcons")
            .join("nav.wav");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, b"RIFF bundled").unwrap();
        let py = obj(json!({"earcons": {
            "session_change": mine.display().to_string(),
            "nav": bundled.display().to_string(),
            "turn_done": legacy.join("gone.wav").display().to_string(),
            "ready": mine.display().to_string(),
        }}));
        let notes = import_earcons(&py, &home, &legacy);
        let dir = home.join("earcons");
        assert_eq!(
            std::fs::read(dir.join("session_change.wav")).unwrap(),
            b"RIFF mine"
        );
        assert!(!dir.join("nav.wav").exists(), "bundled paths are skipped");
        assert!(!dir.join("turn_done.wav").exists());
        assert!(notes.iter().any(|n| n.contains("missing")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("'ready'")), "{notes:?}");
        // A file already in the folder is kept.
        std::fs::write(dir.join("session_change.wav"), b"RIFF kept").unwrap();
        import_earcons(&py, &home, &legacy);
        assert_eq!(
            std::fs::read(dir.join("session_change.wav")).unwrap(),
            b"RIFF kept"
        );
    }

    #[test]
    fn the_background_policy_is_imported_with_the_python_meaning() {
        let dir = tmp();
        for (py, want) in [
            (json!("all"), Some(json!("all"))),
            (json!("anything"), Some(json!("all"))),
            (json!("earcon_only"), Some(json!("earcon_only"))),
        ] {
            let (out, _) =
                convert_config(&obj(json!({"_format": 2, "background_policy": py})), &dir);
            assert_eq!(out.get("background_policy").cloned(), want, "{py}");
        }
    }

    #[test]
    fn the_plugin_config_maps_to_the_runtime_keys() {
        let dir = tmp();
        let (out, notes) = convert_config(
            &obj(json!({
                "_format": 2, "voice": "kokoro:af_sarah", "rate": 250, "volume": 150,
                "audio_mode": "duck", "duck_level": 20, "mute_level": 1,
                "verbosity": "medium", "minqueue": 3, "summary_mode": true,
                "summary_command": "codex", "summary_model": "gpt-5.4-mini",
                "summary_timeout": 90, "summary_settle_ms": 600, "summary_style": "brief",
                "summary_prompts": {"brief": "Say it short.", "poem": "x"},
                "cue_voice": "af_heart", "background_policy": "earcon_only"
            })),
            &dir,
        );
        assert_eq!(
            Value::Object(out),
            json!({
                "voice": "af_sarah", "rate": 250, "audio_mode": "duck",
                "duck_level": 20, "mute_level": 1, "verbosity": "medium", "minqueue": 3,
                "background_policy": "earcon_only", "volume": 100,
                "summaries": {"enabled": true, "command": "codex", "model": "gpt-5.4-mini",
                              "timeout": 90, "style": "brief",
                              "prompts": {"brief": "Say it short."}}
            })
        );
        assert!(notes.iter().any(|n| n.contains("150")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("cue voice")), "{notes:?}");
    }

    #[test]
    fn an_old_format_file_drops_the_old_defaults_and_maps_audio_control() {
        let dir = tmp();
        let (out, _) = convert_config(
            &obj(
                json!({"audio_control": true, "duck_level": 20, "summary_timeout": 20,
                        "rate": 200, "volume": 100, "verbosity": "everything"}),
            ),
            &dir,
        );
        assert_eq!(Value::Object(out), json!({"audio_mode": "duck"}));
    }

    #[test]
    fn chatterbox_voices_speak_as_heart() {
        let dir = tmp();
        std::fs::create_dir_all(dir.join("voices").join("chatterbox")).unwrap();
        std::fs::write(
            dir.join("voices").join("chatterbox").join("Narrator.wav"),
            b"",
        )
        .unwrap();
        for v in ["cb_default", "chatterbox:anything", "narrator"] {
            assert_eq!(map_voice(v, &dir).as_deref(), Some("af_heart"), "{v}");
        }
        assert_eq!(map_voice("af_bella", &dir).as_deref(), Some("af_bella"));
        assert_eq!(
            map_voice("Microsoft Zira", &dir).as_deref(),
            Some("Microsoft Zira")
        );
        assert_eq!(map_voice(" ", &dir), None);
    }

    #[test]
    fn the_plugin_keymap_uses_the_runtime_names() {
        let (km, notes) = convert_keymap(&obj(json!({
            "nav_start": {"key": "home", "mods": ["win", "alt"]},
            "next_session": {"key": "n", "mods": ["Ctrl", "alt"]},
            "flush": {"key": null, "mods": []},
            "mute": {"key": "f13", "mods": ["ctrl", "alt"]},
            "nav_prev": {"key": "left", "mods": ["ctrl", "alt"]}
        })));
        assert_eq!(km["restart"], Binding::new("home", &["win", "alt"]));
        assert_eq!(km["next_channel"], Binding::new("n", &["ctrl", "alt"]));
        assert_eq!(km["flush"], Binding::unbound());
        assert!(!km.contains_key("mute"));
        assert_eq!(notes.len(), 2, "{notes:?}");
    }

    #[test]
    fn run_imports_once_and_never_touches_the_plugin_folder() {
        let legacy = tmp();
        let home = tmp();
        let cfg = r#"{"_format": 2, "rate": 250, "voice": "af_sarah"}"#;
        std::fs::write(legacy.join("config.json"), cfg).unwrap();
        std::fs::write(
            legacy.join("keymap.json"),
            r#"{"mute": {"key": "k", "mods": ["ctrl", "alt"]}}"#,
        )
        .unwrap();
        std::fs::write(
            legacy.join("session_prefs.json"),
            r#"{"abc": {"name": "Build", "voice": "cb_default"}}"#,
        )
        .unwrap();
        let notes = run(&home, &legacy).expect("migrated");
        assert!(notes[0].contains("importing"), "{notes:?}");
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(saved["rate"], 250);
        assert_eq!(saved["voice"], "af_sarah");
        assert!(saved[config::MARKER].is_object());
        let km = keymap::load(&home.join("keymap.json"));
        assert_eq!(km.get(Action::Mute), &Binding::new("k", &["ctrl", "alt"]));
        let prefs: Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("session_prefs.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(prefs["abc"], json!({"label": "Build", "voice": "af_heart"}));
        assert_eq!(
            std::fs::read_to_string(legacy.join("config.json")).unwrap(),
            cfg
        );
        assert_eq!(run(&home, &legacy), None, "idempotent");
    }

    #[test]
    fn nothing_to_migrate_writes_nothing() {
        let legacy = tmp();
        let home = tmp();
        assert_eq!(run(&home, &legacy.join("missing")), None);
        assert!(!home.join("config.json").exists());
    }
}
