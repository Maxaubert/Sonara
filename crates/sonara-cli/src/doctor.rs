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

/// The hotkeys row from `get hotkeys`.
pub fn hotkeys_row(h: &Value) -> Row {
    let bindings = h["bindings"].as_array().cloned().unwrap_or_default();
    let mut bound = Vec::new();
    let mut problems = Vec::new();
    for b in &bindings {
        let (Some(action), Some(combo)) = (b["action"].as_str(), b["combo"].as_str()) else {
            continue;
        };
        bound.push(format!("{combo} {action}"));
        if b["error"].as_str() == Some("already_owned") {
            problems.push(format!("{combo} is taken by another program"));
        } else if let Some(err) = b["error"].as_str() {
            problems.push(format!("{combo}: {err}"));
        }
        if let Some(ch) = b["altgr"].as_str() {
            problems.push(format!(
                "{combo} types '{ch}' with AltGr on this keyboard; rebind it with Win in the settings page"
            ));
        }
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
    let list = if bound.is_empty() {
        "none bound".to_string()
    } else {
        bound.join(", ")
    };
    let status = if h["active"] == true {
        Status::Ok
    } else {
        Status::Info
    };
    Row::new(status, "hotkeys", format!("{list}{active}"))
}

/// The audio row from `audio_mode` and `duck_level`.
pub fn audio_row(mode: &Value, level: &Value) -> Row {
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
        let r = hotkeys_row(&ok);
        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "Ctrl+Alt+Up restart")
        );
        let bad = json!({"active": true, "bindings": [
            {"action": "mute", "combo": "Ctrl+Alt+M", "registered": true, "error": null, "altgr": "µ"},
            {"action": "flush", "combo": "Ctrl+Alt+Down", "registered": false, "error": "already_owned", "altgr": null}],
            "problems": []});
        let r = hotkeys_row(&bad);
        assert_eq!(r.status, Status::Warn);
        assert!(
            r.detail.contains("AltGr") && r.detail.contains("taken"),
            "{}",
            r.detail
        );
    }

    #[test]
    fn the_audio_mode_is_described() {
        assert!(audio_row(&json!("duck"), &json!(40))
            .detail
            .contains("40 %"));
        assert!(audio_row(&json!("pause"), &json!(30))
            .detail
            .contains("paused"));
        assert_eq!(audio_row(&json!("off"), &json!(30)).status, Status::Info);
    }
}
