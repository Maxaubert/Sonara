//! The activity lines L4 reports to its host (#217): which apps were
//! paused, resumed, ducked or restored, and why. One short line each, with
//! a fixed prefix (`media pause`, `media resume`, `duck`, `restore`,
//! `startup sweep`) so a support log can be grepped. Never any spoken text:
//! only app names, item ids and session labels.
use std::sync::Arc;

/// Where activity lines go (the host's support log). Without one they go
/// to stderr.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Write `line` to `log`, else to stderr.
pub fn emit(log: Option<&LogFn>, line: &str) {
    match log {
        Some(f) => f(line),
        None => eprintln!("sonara-system: {line}"),
    }
}

/// A value for a `key=value` field: as is, or quoted when it holds a
/// space, a quote or nothing, so every field stays one token.
pub fn value(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"' || c == '=') {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// App names as one field value: `a,b`, or `none`.
pub fn apps<S: AsRef<str>>(names: &[S]) -> String {
    if names.is_empty() {
        return "none".into();
    }
    names
        .iter()
        .map(|n| value(n.as_ref()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Why other apps are engaged: the item being read (L1 state), with its
/// label (a channel's session label) when it has one.
pub fn reading_reason(state: &sonara_reader::State) -> Option<String> {
    let np = state.now_playing.as_ref()?;
    Some(match np.label.as_deref().filter(|l| !l.is_empty()) {
        Some(l) => format!("reading item={} session={}", np.item_id.0, value(l)),
        None => format!("reading item={}", np.item_id.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_stay_one_token() {
        assert_eq!(value("vlc.exe"), "vlc.exe");
        assert_eq!(value("my session"), "\"my session\"");
        assert_eq!(value("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(value(""), "\"\"");
        assert_eq!(value("a\nb"), "\"a b\"");
    }

    #[test]
    fn app_lists_are_comma_joined_or_none() {
        assert_eq!(apps::<&str>(&[]), "none");
        assert_eq!(
            apps(&["spotify.exe", "chrome.exe"]),
            "spotify.exe,chrome.exe"
        );
    }
}
