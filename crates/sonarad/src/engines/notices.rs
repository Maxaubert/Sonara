//! The fallback notices of spec 8.3 as `sonarad.log` lines.
use super::*;

/// The notice line of spec 8.3, or `None` while the gate holds it back.
pub fn notice_line(n: &Notice) -> String {
    match n.reason {
        None => format!("engine {} recovered", n.engine),
        Some(r) => format!(
            "engine {} fallback reason={}{} -> {}: {}",
            n.engine,
            r.as_str(),
            n.status.map(|s| format!(" status={s}")).unwrap_or_default(),
            n.fallback.map(|f| f.as_str()).unwrap_or("none"),
            sonara_log::mask(&n.message)
        ),
    }
}

impl Engines {
    pub(super) fn note(&self, line: &str) {
        sonara_system::log::emit(self.log.as_ref(), line);
    }
}
