//! What `logs\sonarad.log` records for support besides the rare notes
//! (migration, settings that could not apply, custom earcons): one line at
//! every start (version, process, engine and its model status, home) and a
//! line whenever the engine's readiness changes (the Kokoro model finished
//! downloading or loading, or failed). Download progress is not logged.
//!
//! The activity lines (#217), one per event and never any spoken text:
//! - `read start item=<id> session=<label|channel|direct> chunks=<n>`
//!   (` kind=announce` for a session switch announcement) and `read end
//!   item=<id> finished|skipped|failed`, from the reader's events;
//! - `ask kind=question|permission|plan session=<label>` when a decision
//!   arrives (it is read next, ahead of the session's other text);
//! - `hotkey <action>` with what it did (`hotkey mute level=2`);
//! - other apps' audio, from `sonara_system` (`media pause apps=...
//!   (reason: reading item=<id> session=<label>)`, `media resume`, `duck`,
//!   `restore`, `startup sweep`): the item id ties them to `read start`.
use crate::home::Home;
use sonara_channels::Tag;
use sonara_engine::{EngineStatus, Readiness};
use sonara_reader::{Event, ItemId, ItemPhase, ReaderHandle};
use sonara_system::log::value;
use std::collections::HashSet;

/// The line written once at startup.
pub fn startup_line(version: &str, pid: u32, engine: &str, status: &EngineStatus) -> String {
    format!("sonarad {version} started (pid {pid}): engine {engine} {status}")
}

/// Log the startup line now.
pub fn log_startup(home: &Home, version: &str, reader: &ReaderHandle, engine: &str) {
    let status = reader
        .engine_status()
        .unwrap_or_else(|_| EngineStatus::ready());
    home.log(&format!(
        "{}; home {}",
        startup_line(version, std::process::id(), engine, &status),
        home.dir.display()
    ));
}

/// Log the engine's readiness changes from now on, on a thread that ends
/// with the reader.
pub fn watch_engine(home: &Home, reader: &ReaderHandle) {
    let Ok(events) = reader.subscribe() else {
        return;
    };
    let mut last: Option<Readiness> = reader.engine_status().ok().map(|s| s.readiness);
    let home = home.clone();
    let _ = std::thread::Builder::new()
        .name("sonarad-support-log".into())
        .spawn(move || {
            while let Ok(e) = events.recv() {
                if let Event::EngineStatus { engine, status, .. } = e {
                    if last != Some(status.readiness) {
                        last = Some(status.readiness);
                        home.log(&format!("engine {engine}: {status}"));
                    }
                }
            }
        });
}

/// Which session an item belongs to: its label, else its channel, else
/// `direct` (a plain `speak`).
pub fn session_of(label: Option<&str>, tag: Option<&Tag>) -> String {
    match (label.filter(|l| !l.is_empty()), tag) {
        (Some(l), _) => l.to_string(),
        (None, Some(t)) => t.channel.clone(),
        (None, None) => "direct".into(),
    }
}

/// `read start item=<id> session=<s> chunks=<n>[ kind=announce]`.
pub fn read_start_line(item: u64, session: &str, chunks: usize, announce: bool) -> String {
    format!(
        "read start item={item} session={} chunks={chunks}{}",
        value(session),
        if announce { " kind=announce" } else { "" }
    )
}

/// `read end item=<id> finished|skipped|failed`, or `None` for `Started`.
pub fn read_end_line(item: u64, phase: ItemPhase) -> Option<String> {
    let how = match phase {
        ItemPhase::Started => return None,
        ItemPhase::Finished => "finished",
        ItemPhase::Skipped => "skipped",
        ItemPhase::Failed => "failed",
    };
    Some(format!("read end item={item} {how}"))
}

/// `ask kind=<kind> session=<s>`: a decision arrived.
pub fn ask_line(kind: &str, session: &str) -> String {
    format!("ask kind={} session={}", value(kind), value(session))
}

/// `hotkey <action>[ <detail>]`, or `hotkey <action> failed: <error>`.
pub fn hotkey_line(action: &str, outcome: &Result<Option<String>, String>) -> String {
    match outcome {
        Ok(None) => format!("hotkey {action}"),
        Ok(Some(d)) => format!("hotkey {action} {d}"),
        Err(e) => format!("hotkey {action} failed: {e}"),
    }
}

/// Which channel fed an item (`Channels::tag`), when the channels
/// extension is on.
pub type TagLookup = Box<dyn Fn(ItemId) -> Option<Tag> + Send>;

/// The reading lines of one event stream: a start when an item first
/// shows in the state (it carries the label and chunk count), an end for
/// every item seen starting, and `reader paused` / `reader resumed` when
/// the state's pause flips, whatever paused it (hotkey, CLI, settings
/// page, SDK). Items dropped from the queue unread write nothing.
pub struct ReadLog {
    tags: TagLookup,
    /// The pause flag of the last state seen.
    paused: bool,
    /// `Started` reported, not yet shown in a state.
    starting: HashSet<u64>,
    /// Start line written, end not yet.
    reading: HashSet<u64>,
}

impl ReadLog {
    pub fn new(tags: TagLookup) -> ReadLog {
        ReadLog {
            tags,
            paused: false,
            starting: HashSet::new(),
            reading: HashSet::new(),
        }
    }

    /// The lines `e` adds to the log, oldest first.
    pub fn lines(&mut self, e: &Event) -> Vec<String> {
        let mut out = Vec::new();
        if let Event::State(s) = e {
            if s.paused != self.paused {
                self.paused = s.paused;
                out.push(
                    if s.paused {
                        "reader paused"
                    } else {
                        "reader resumed"
                    }
                    .to_string(),
                );
            }
        }
        out.extend(self.read_line(e));
        out
    }

    fn read_line(&mut self, e: &Event) -> Option<String> {
        match e {
            Event::State(s) => {
                let np = s.now_playing.as_ref()?;
                let id = np.item_id.0;
                if self.reading.contains(&id) {
                    return None;
                }
                self.starting.remove(&id);
                self.reading.insert(id);
                let tag = (self.tags)(np.item_id);
                Some(read_start_line(
                    id,
                    &session_of(np.label.as_deref(), tag.as_ref()),
                    np.chunks,
                    tag.as_ref().is_some_and(|t| t.announcement),
                ))
            }
            Event::Item { item_id, phase } => {
                let id = item_id.0;
                if *phase == ItemPhase::Started {
                    self.starting.insert(id);
                    return None;
                }
                let seen = self.reading.remove(&id) | self.starting.remove(&id);
                if seen {
                    read_end_line(id, *phase)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Log the reading lines from now on, on a thread that ends with the
/// reader.
pub fn watch_reading(home: &Home, reader: &ReaderHandle, tags: TagLookup) {
    let Ok(events) = reader.subscribe() else {
        return;
    };
    let home = home.clone();
    let _ = std::thread::Builder::new()
        .name("sonarad-read-log".into())
        .spawn(move || {
            let mut log = ReadLog::new(tags);
            while let Ok(e) = events.recv() {
                for line in log.lines(&e) {
                    home.log(&line);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, Registry};
    use std::sync::Arc;

    #[test]
    fn the_startup_line_names_version_engine_and_model_status() {
        let mut s = EngineStatus::ready();
        assert_eq!(
            startup_line("0.11.1", 42, "kokoro", &s),
            "sonarad 0.11.1 started (pid 42): engine kokoro ready"
        );
        s.readiness = Readiness::Downloading;
        s.progress = Some((40, 100));
        let line = startup_line("0.11.1", 42, "kokoro", &s);
        assert!(line.contains("downloading its model (40%)"), "{line}");
    }

    #[test]
    fn reading_lines_have_fixed_fields_and_no_text() {
        assert_eq!(
            read_start_line(12, "Sonara fix", 3, false),
            "read start item=12 session=\"Sonara fix\" chunks=3"
        );
        assert_eq!(
            read_start_line(13, "work", 1, true),
            "read start item=13 session=work chunks=1 kind=announce"
        );
        assert_eq!(
            read_end_line(12, ItemPhase::Finished).unwrap(),
            "read end item=12 finished"
        );
        assert_eq!(
            read_end_line(12, ItemPhase::Skipped).unwrap(),
            "read end item=12 skipped"
        );
        assert_eq!(
            read_end_line(12, ItemPhase::Failed).unwrap(),
            "read end item=12 failed"
        );
        assert!(read_end_line(12, ItemPhase::Started).is_none());
        assert_eq!(
            ask_line("permission", "my session"),
            "ask kind=permission session=\"my session\""
        );
        assert_eq!(hotkey_line("flush", &Ok(None)), "hotkey flush");
        assert_eq!(
            hotkey_line("mute", &Ok(Some("level=2".into()))),
            "hotkey mute level=2"
        );
        assert_eq!(
            hotkey_line("restart", &Err("closed".into())),
            "hotkey restart failed: closed"
        );
    }

    #[test]
    fn the_session_is_the_label_else_the_channel_else_direct() {
        let tag = Tag {
            channel: "c1".into(),
            host_tab: None,
            announcement: false,
        };
        assert_eq!(session_of(Some("work"), Some(&tag)), "work");
        assert_eq!(session_of(Some(""), Some(&tag)), "c1");
        assert_eq!(session_of(None, None), "direct");
    }

    fn state(id: u64, label: Option<&str>, text: &str, chunks: usize) -> Event {
        Event::State(sonara_reader::State {
            seq: 1,
            now_playing: Some(sonara_reader::NowPlaying {
                item_id: ItemId(id),
                label: label.map(str::to_string),
                text: text.into(),
                chunk: 0,
                chunks,
            }),
            queued: 0,
            paused: false,
            muted: false,
            volume: 100,
            rate: 200,
            voice: None,
        })
    }

    fn item(id: u64, phase: ItemPhase) -> Event {
        Event::Item {
            item_id: ItemId(id),
            phase,
        }
    }

    #[test]
    fn one_start_and_one_end_per_item_never_the_text() {
        let mut log = ReadLog::new(Box::new(|id| {
            (id.0 == 2).then(|| Tag {
                channel: "c1".into(),
                host_tab: None,
                announcement: true,
            })
        }));
        let lines: Vec<String> = [
            item(1, ItemPhase::Started),
            state(1, None, "Secret words.", 2),
            state(1, None, "More secret words.", 2),
            item(1, ItemPhase::Finished),
            item(2, ItemPhase::Started),
            state(2, Some("work"), "Session changed: work.", 1),
            item(2, ItemPhase::Skipped),
            // Dropped from the queue unread: nothing.
            item(3, ItemPhase::Skipped),
        ]
        .iter()
        .flat_map(|e| log.lines(e))
        .collect();
        assert_eq!(
            lines,
            vec![
                "read start item=1 session=direct chunks=2",
                "read end item=1 finished",
                "read start item=2 session=work chunks=1 kind=announce",
                "read end item=2 skipped",
            ]
        );
        assert!(lines.iter().all(|l| !l.contains("ecret")));
    }

    fn paused_state(id: Option<u64>, paused: bool) -> Event {
        let Event::State(mut s) = state(id.unwrap_or(0), None, "Secret.", 1) else {
            unreachable!()
        };
        if id.is_none() {
            s.now_playing = None;
        }
        s.paused = paused;
        Event::State(s)
    }

    #[test]
    fn a_reader_pause_and_resume_is_logged_whatever_its_source() {
        let mut log = ReadLog::new(Box::new(|_| None));
        let lines: Vec<String> = [
            paused_state(None, false),
            // Paused before anything plays (a protocol `control`).
            paused_state(None, true),
            paused_state(None, true),
            // The start and the resume arrive in one state.
            paused_state(Some(1), false),
            paused_state(Some(1), true),
            paused_state(Some(1), false),
        ]
        .iter()
        .flat_map(|e| log.lines(e))
        .collect();
        assert_eq!(
            lines,
            vec![
                "reader paused",
                "reader resumed",
                "read start item=1 session=direct chunks=1",
                "reader paused",
                "reader resumed",
            ]
        );
    }

    #[test]
    fn startup_creates_the_log_file() {
        let dir = std::env::temp_dir().join(format!("sonarad-support-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home {
            dir: dir.clone(),
            is_default: false,
        };
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        log_startup(&home, "9.9.9", &reader, "fake");
        let text = std::fs::read_to_string(home.log_path()).unwrap();
        assert!(
            text.contains("sonarad 9.9.9 started") && text.contains("engine fake ready"),
            "{text}"
        );
        assert!(text.contains(&dir.display().to_string()), "{text}");
    }
}
