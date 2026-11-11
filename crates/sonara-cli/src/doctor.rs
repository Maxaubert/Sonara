//! `sonara doctor`: one row per check. Only a broken install or a runtime
//! that cannot start is `FAIL`; what is merely worth knowing is `INFO`,
//! and what the user may want to fix is `WARN`.
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::Ok => " OK ",
            Status::Info => "INFO",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub status: Status,
    pub label: String,
    pub detail: String,
}

impl Row {
    pub fn new(status: Status, label: &str, detail: impl Into<String>) -> Row {
        Row {
            status,
            label: label.to_string(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.status.tag(), self.label, self.detail)
    }
}

/// Whether the report holds a failure (the exit code is then 1).
pub fn failed(rows: &[Row]) -> bool {
    rows.iter().any(|r| r.status == Status::Fail)
}

fn mb(bytes: u64) -> u64 {
    (bytes + 500_000) / 1_000_000
}

/// The engine row from `state.engine_status`.
pub fn engine_row(status: &Value) -> Row {
    let engine = status["engine"].as_str().unwrap_or("unknown");
    let fallback = status["fallback"]
        .as_str()
        .map(|f| format!("; speaking with {f} meanwhile"))
        .unwrap_or_default();
    let message = status["message"]
        .as_str()
        .map(|m| format!(" ({m})"))
        .unwrap_or_default();
    match status["status"].as_str().unwrap_or("ready") {
        "ready" => Row::new(Status::Ok, "engine", format!("{engine}, ready")),
        "loading" => Row::new(
            Status::Info,
            "engine",
            format!("{engine} is loading its voice model{fallback}"),
        ),
        "downloading" => {
            let p = &status["progress"];
            let progress = match (p["done"].as_u64(), p["total"].as_u64()) {
                (Some(d), Some(t)) if t > 0 => format!(": {} of {} MB", mb(d), mb(t)),
                _ => String::new(),
            };
            Row::new(
                Status::Info,
                "engine",
                format!("{engine} is downloading its voice model{progress}{fallback}"),
            )
        }
        "waiting" => Row::new(
            Status::Warn,
            "engine",
            format!(
                "{engine}: the voice model download failed and is retried later{message}{fallback}"
            ),
        ),
        other => Row::new(
            Status::Warn,
            "engine",
            format!("{engine} is {other}{message}{fallback}"),
        ),
    }
}

/// The voices row from a `voices` reply: how many each engine lists.
pub fn voices_row(voices: &Value) -> Row {
    let list = voices.as_array().cloned().unwrap_or_default();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for v in &list {
        let e = v["engine"].as_str().unwrap_or("?").to_string();
        match counts.iter_mut().find(|(n, _)| *n == e) {
            Some((_, c)) => *c += 1,
            None => counts.push((e, 1)),
        }
    }
    if counts.is_empty() {
        return Row::new(Status::Warn, "voices", "no engine lists a voice");
    }
    let mut detail = counts
        .iter()
        .map(|(e, c)| format!("{c} {e}"))
        .collect::<Vec<_>>()
        .join(", ");
    let has = |name: &str| counts.iter().any(|(e, _)| e == name);
    if has("kokoro") && !has("onecore") {
        detail.push_str(" (Windows lists no OneCore voice for Sonara on this PC; Kokoro only)");
    }
    Row::new(Status::Info, "voices", detail)
}

/// Said once when a Ctrl+Alt+arrow hotkey is taken (#283): Intel graphics
/// drivers rotate the screen with those chords.
pub const INTEL_ROTATION: &str = "Intel graphics uses Ctrl+Alt+arrows to rotate the screen: \
turn its hotkeys off in Intel Graphics Command Center, or rebind with Win in the settings page";

/// `Ctrl+Alt+Up`, `Down`, `Left` or `Right` (no other modifier).
fn ctrl_alt_arrow(combo: &str) -> bool {
    ["Up", "Down", "Left", "Right"]
        .iter()
        .any(|k| combo == format!("Ctrl+Alt+{k}"))
}

/// Said once on macOS when a Ctrl chord with an arrow is taken: Mission
/// Control and Spaces use Ctrl+arrows.
pub const MISSION_CONTROL: &str = "macOS may use it for Mission Control or Spaces: change that \
in System Settings, Keyboard, Keyboard Shortcuts, or rebind with Cmd in the settings page";

/// Said on macOS while a hotkey is Ctrl+Option (the defaults, user decision
/// 2026-11-11): VoiceOver's modifier.
pub const VOICEOVER: &str = "Ctrl+Option is the VoiceOver modifier: if you use VoiceOver, \
rebind with Cmd in the settings page";

/// A Ctrl chord (not Cmd) on an arrow, as macOS writes it.
fn ctrl_arrow(combo: &str) -> bool {
    combo.starts_with("Ctrl+")
        && !combo.contains("Cmd")
        && ["Up", "Down", "Left", "Right"]
            .iter()
            .any(|k| combo.ends_with(&format!("+{k}")))
}

/// The hotkeys row from `get hotkeys`.
pub fn hotkeys_row(h: &Value) -> Row {
    hotkeys_row_for(h, cfg!(target_os = "macos"))
}

/// `hotkeys_row` with the texts of macOS (`mac`) or Windows.
pub fn hotkeys_row_for(h: &Value, mac: bool) -> Row {
    let bindings = h["bindings"].as_array().cloned().unwrap_or_default();
    let mut bound = Vec::new();
    let mut problems = Vec::new();
    let mut rotation = false;
    let mut mission_control = false;
    let mut voiceover = false;
    for b in &bindings {
        let (Some(action), Some(combo)) = (b["action"].as_str(), b["combo"].as_str()) else {
            continue;
        };
        bound.push(format!("{combo} {action}"));
        voiceover |= mac && combo.starts_with("Ctrl+Option+") && !combo.contains("Cmd");
        if b["error"].as_str() == Some("already_owned") {
            problems.push(format!("{combo} is taken by another program"));
            rotation |= !mac && ctrl_alt_arrow(combo);
            mission_control |= mac && ctrl_arrow(combo);
        } else if b["error"].as_str() == Some("bound_twice") {
            // #43: one of our own actions holds the chord.
            let other = b["taken_by"].as_str().unwrap_or("another action");
            problems.push(format!(
                "{combo} {action} is also bound to {other}; rebind one in the settings page"
            ));
        } else if let Some(err) = b["error"].as_str() {
            problems.push(format!("{combo}: {err}"));
        }
        if let Some(ch) = b["altgr"].as_str() {
            problems.push(format!(
                "{combo} types '{ch}' with AltGr on this keyboard; rebind it with Win in the settings page"
            ));
        }
    }
    if rotation {
        problems.push(INTEL_ROTATION.to_string());
    }
    if mission_control {
        problems.push(MISSION_CONTROL.to_string());
    }
    for p in h["problems"].as_array().into_iter().flatten() {
        problems.push(format!("keymap.json: {}", p.as_str().unwrap_or("?")));
    }
    if !problems.is_empty() {
        return Row::new(Status::Warn, "hotkeys", problems.join("; "));
    }
    let active = if h["active"] == true {
        ""
    } else {
        " (not registered now)"
    };
    let mut list = if bound.is_empty() {
        "none bound".to_string()
    } else {
        bound.join(", ")
    };
    if voiceover {
        list = format!("{list} ({VOICEOVER})");
    }
    let status = if h["active"] == true {
        Status::Ok
    } else {
        Status::Info
    };
    Row::new(status, "hotkeys", format!("{list}{active}"))
}

/// The audio row from `audio_mode` and `duck_level`.
pub fn audio_row(mode: &Value, level: &Value) -> Row {
    audio_row_for(mode, level, cfg!(target_os = "macos"))
}

/// `audio_row` as on macOS (`mac`: no duck, and pause reaches only the
/// scriptable players) or Windows.
pub fn audio_row_for(mode: &Value, level: &Value, mac: bool) -> Row {
    if mac {
        return match mode.as_str() {
            Some("duck") => Row::new(
                Status::Warn,
                "audio",
                "ducking is not available on macOS: Music and Spotify are paused instead \
                 (choose Pause or Off in the settings page)",
            ),
            Some("pause") => Row::new(
                Status::Info,
                "audio",
                "Music and Spotify are paused while Sonara speaks (other media is left alone)",
            ),
            _ => audio_row_for(mode, level, false),
        };
    }
    let detail = match mode.as_str() {
        Some("pause") => "media apps are paused while Sonara speaks".to_string(),
        Some("duck") => format!(
            "other apps are lowered to {} % while Sonara speaks",
            level.as_u64().unwrap_or(30)
        ),
        Some("off") => "other apps' audio is left alone".to_string(),
        _ => "unknown".to_string(),
    };
    Row::new(Status::Info, "audio", detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_render_with_their_status() {
        let r = Row::new(Status::Ok, "runtime", "running");
        assert_eq!(r.to_string(), "[ OK ] runtime: running");
        assert!(!failed(&[r.clone(), Row::new(Status::Warn, "x", "y")]));
        assert!(failed(&[r, Row::new(Status::Fail, "x", "y")]));
    }

    #[test]
    fn a_downloading_model_is_information_not_a_failure() {
        let r = engine_row(
            &json!({"engine": "kokoro", "ready": false, "status": "downloading",
            "progress": {"done": 104_873_984u64, "total": 353_746_785u64}, "fallback": "onecore"}),
        );
        assert_eq!(r.status, Status::Info);
        assert_eq!(
            r.detail,
            "kokoro is downloading its voice model: 105 of 354 MB; speaking with onecore meanwhile"
        );
        let r = engine_row(&json!({"engine": "kokoro", "ready": true, "status": "ready"}));
        assert_eq!(r.status, Status::Ok);
        let r = engine_row(&json!({"engine": "kokoro", "status": "waiting", "message": "offline"}));
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("offline"), "{}", r.detail);
    }

    #[test]
    fn voices_are_counted_per_engine() {
        let r =
            voices_row(&json!([{"engine": "kokoro"}, {"engine": "kokoro"}, {"engine": "onecore"}]));
        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Info, "2 kokoro, 1 onecore")
        );
        let r = voices_row(&json!([{"engine": "kokoro"}]));
        assert!(r.detail.contains("Kokoro only"), "{}", r.detail);
        assert_eq!(voices_row(&json!([])).status, Status::Warn);
    }

    #[test]
    fn altgr_and_taken_hotkeys_are_warnings() {
        let ok = json!({"active": true, "bindings": [
            {"action": "restart", "combo": "Ctrl+Alt+Up", "registered": true, "error": null, "altgr": null},
            {"action": "pause", "combo": null}], "problems": []});
        let r = hotkeys_row_for(&ok, false);
        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "Ctrl+Alt+Up restart")
        );
        let bad = json!({"active": true, "bindings": [
            {"action": "mute", "combo": "Ctrl+Alt+M", "registered": true, "error": null, "altgr": "µ"},
            {"action": "flush", "combo": "Ctrl+Alt+Down", "registered": false, "error": "already_owned", "altgr": null}],
            "problems": []});
        let r = hotkeys_row_for(&bad, false);
        assert_eq!(r.status, Status::Warn);
        assert!(
            r.detail.contains("AltGr") && r.detail.contains("taken"),
            "{}",
            r.detail
        );
    }

    #[test]
    fn a_chord_bound_twice_names_the_other_action_not_another_program() {
        // #43
        let h = json!({"active": true, "bindings": [
            {"action": "mute", "combo": "Ctrl+Alt+M", "error": null},
            {"action": "faster", "combo": "Ctrl+Alt+M", "error": "bound_twice", "taken_by": "mute"}],
            "problems": []});
        let r = hotkeys_row(&h);
        assert_eq!(r.status, Status::Warn);
        assert!(
            r.detail.contains("Ctrl+Alt+M faster is also bound to mute"),
            "{}",
            r.detail
        );
        assert!(!r.detail.contains("another program"), "{}", r.detail);
    }

    #[test]
    fn a_taken_ctrl_alt_arrow_names_intel_rotation() {
        let h = json!({"active": true, "bindings": [
            {"action": "previous_question", "combo": "Ctrl+Alt+Left", "error": "already_owned"},
            {"action": "next_question", "combo": "Ctrl+Alt+Right", "error": "already_owned"}],
            "problems": []});
        let r = hotkeys_row_for(&h, false);
        assert_eq!(r.status, Status::Warn);
        assert_eq!(
            r.detail.matches("Intel Graphics Command Center").count(),
            1,
            "{}",
            r.detail
        );
        // Another taken chord says nothing about Intel.
        let h = json!({"active": true, "bindings": [
            {"action": "mute", "combo": "Ctrl+Alt+M", "error": "already_owned"},
            {"action": "next_question", "combo": "Win+Ctrl+Alt+Right", "error": "already_owned"}],
            "problems": []});
        assert!(!hotkeys_row_for(&h, false).detail.contains("Intel"));
    }

    #[test]
    fn macos_hotkeys_name_voiceover_and_mission_control_not_intel() {
        let ok = json!({"active": true, "bindings": [
            {"action": "restart", "combo": "Ctrl+Option+Up", "registered": true, "error": null}],
            "problems": []});
        let r = hotkeys_row_for(&ok, true);
        assert_eq!(r.status, Status::Ok, "a note, not a warning");
        assert!(r
            .detail
            .starts_with("Ctrl+Option+Up restart (Ctrl+Option is the VoiceOver"));
        let cmd = json!({"active": true, "bindings": [
            {"action": "restart", "combo": "Ctrl+Option+Cmd+Up", "error": null}], "problems": []});
        assert!(!hotkeys_row_for(&cmd, true).detail.contains("VoiceOver"));
        let taken = json!({"active": true, "bindings": [
            {"action": "flush", "combo": "Ctrl+Option+Down", "error": "already_owned"}],
            "problems": []});
        let r = hotkeys_row_for(&taken, true);
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("Mission Control"), "{}", r.detail);
        assert!(!r.detail.contains("Intel"), "{}", r.detail);
    }

    #[test]
    fn macos_audio_says_duck_is_unavailable_and_what_pause_reaches() {
        let r = audio_row_for(&json!("duck"), &json!(40), true);
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("not available on macOS"), "{}", r.detail);
        let r = audio_row_for(&json!("pause"), &json!(30), true);
        assert!(r.detail.contains("Music and Spotify"), "{}", r.detail);
        assert_eq!(
            audio_row_for(&json!("off"), &json!(30), true).status,
            Status::Info
        );
    }

    #[test]
    fn the_audio_mode_is_described() {
        assert!(audio_row_for(&json!("duck"), &json!(40), false)
            .detail
            .contains("40 %"));
        assert!(audio_row(&json!("pause"), &json!(30))
            .detail
            .contains("paused"));
        assert_eq!(audio_row(&json!("off"), &json!(30)).status, Status::Info);
    }
}
