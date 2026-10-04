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
//!
//! The troubleshooting lines of #219 (`trace_log` has the others):
//! - `read text item=<id> session=<s> kind=<k> from=<message>
//!   chunks=<read>/<all> text="..."` right before each `read end`: the
//!   exact text that went to the voice (the cleaned chunks read, joined),
//!   what it is (`prose`, `question`, `permission`, `plan`, `tool`,
//!   `summary`, `announce`, `speak`) and the message that produced it;
//!   `text` only with the setting `debug_log` on;
//! - `read drop item=<id> ... unread`: an item the reader dropped from its
//!   queue without reading it (replaced, stopped, nothing speakable).
use crate::home::Home;
use crate::trace_log::{text_field, Origins};
use sonara_channels::Tag;
use sonara_engine::{EngineStatus, Readiness};
use sonara_reader::{Event, ItemId, ItemPhase, ReaderHandle};
use sonara_system::log::value;
use std::collections::{BTreeMap, HashMap, HashSet};

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

/// An item being read: where it came from and the chunks heard so far.
struct Reading {
    session: String,
    /// Its kind and source are looked up when it ends: the agent records
    /// an entry's origin only after the channel fed it to the reader.
    tag: Option<Tag>,
    total: usize,
    chunks: BTreeMap<usize, String>,
}

/// The reading lines of one event stream: a start when an item first
/// shows in the state (it carries the label and chunk count), its text
/// and an end for every item seen starting, a drop for an item that ended
/// unread, and `reader paused` / `reader resumed` when the state's pause
/// flips, whatever paused it (hotkey, CLI, settings page, SDK).
pub struct ReadLog {
    tags: TagLookup,
    origins: Origins,
    /// The pause flag of the last state seen.
    paused: bool,
    /// `Started` reported, not yet shown in a state.
    starting: HashSet<u64>,
    /// Start line written, end not yet.
    reading: HashMap<u64, Reading>,
}

/// (kind, source) of an item from its tag: an announcement, a channel
/// entry the agent added (`origins`), else text a client spoke.
fn origin_of(tag: Option<&Tag>, origins: &Origins) -> (String, String) {
    match tag {
        Some(t) if t.announcement => ("announce".into(), "switch".into()),
        Some(t) => t
            .entry
            .and_then(|e| origins.get(e))
            .map(|o| (o.kind, o.source))
            .unwrap_or_else(|| ("speak".into(), "speak".into())),
        None => ("speak".into(), "speak".into()),
    }
}

/// `read text item=<id> session=<s> kind=<k> from=<f> chunks=<n>/<all>[
/// text="..."]`.
pub fn read_text_line(
    item: u64,
    session: &str,
    kind: &str,
    from: &str,
    chunks: &[&str],
    total: usize,
    debug: bool,
) -> String {
    format!(
        "read text item={item} session={} kind={kind} from={} chunks={}/{total}{}",
        value(session),
        value(from),
        chunks.len(),
        text_field(&chunks.join(" "), debug)
    )
}

impl ReadLog {
    pub fn new(tags: TagLookup, origins: Origins) -> ReadLog {
        ReadLog {
            tags,
            origins,
            paused: false,
            starting: HashSet::new(),
            reading: HashMap::new(),
        }
    }

    /// The lines `e` adds to the log, oldest first; `debug` writes the
    /// spoken text.
    pub fn lines(&mut self, e: &Event, debug: bool) -> Vec<String> {
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
        self.read_lines(e, debug, &mut out);
        out
    }

    fn read_lines(&mut self, e: &Event, debug: bool, out: &mut Vec<String>) {
        match e {
            Event::State(s) => {
                let Some(np) = s.now_playing.as_ref() else {
                    return;
                };
                let id = np.item_id.0;
                if let Some(r) = self.reading.get_mut(&id) {
                    r.chunks.insert(np.chunk, np.text.clone());
                    return;
                }
                self.starting.remove(&id);
                let tag = (self.tags)(np.item_id);
                let session = session_of(np.label.as_deref(), tag.as_ref());
                out.push(read_start_line(
                    id,
                    &session,
                    np.chunks,
                    tag.as_ref().is_some_and(|t| t.announcement),
                ));
                self.reading.insert(
                    id,
                    Reading {
                        session,
                        tag,
                        total: np.chunks,
                        chunks: BTreeMap::from([(np.chunk, np.text.clone())]),
                    },
                );
            }
            Event::Item { item_id, phase } => {
                let id = item_id.0;
                if *phase == ItemPhase::Started {
                    self.starting.insert(id);
                    return;
                }
                if let Some(mut r) = self.reading.remove(&id) {
                    if r.tag.is_none() {
                        // The channel may have tagged it after its first
                        // state was seen.
                        r.tag = (self.tags)(*item_id);
                        if let Some(tag) = &r.tag {
                            r.session = session_of(None, Some(tag));
                        }
                    }
                    let (kind, from) = origin_of(r.tag.as_ref(), &self.origins);
                    let chunks: Vec<&str> = r.chunks.values().map(String::as_str).collect();
                    out.push(read_text_line(
                        id, &r.session, &kind, &from, &chunks, r.total, debug,
                    ));
                    out.extend(read_end_line(id, *phase));
                } else if self.starting.remove(&id) {
                    out.extend(read_end_line(id, *phase));
                } else {
                    let tag = (self.tags)(*item_id);
                    let (kind, from) = origin_of(tag.as_ref(), &self.origins);
                    out.push(format!(
                        "read drop item={id} session={} kind={kind} from={} unread",
                        value(&session_of(None, tag.as_ref())),
                        value(&from)
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Log every spoken control cue (`cue text="Paused."`, #219) from now on,
/// on a thread that ends with the cue worker.
pub fn watch_cues(home: &Home, cues: &crate::cues::Cues) {
    let stream = cues.subscribe();
    let home = home.clone();
    let _ = std::thread::Builder::new()
        .name("sonarad-cue-log".into())
        .spawn(move || {
            while let Ok(text) = stream.recv() {
                home.log(&crate::trace_log::cue_line(&text));
            }
        });
}

/// Log the reading lines from now on, on a thread that ends with the
/// reader.
pub fn watch_reading(home: &Home, reader: &ReaderHandle, tags: TagLookup, origins: Origins) {
    let Ok(events) = reader.subscribe() else {
        return;
    };
    let home = home.clone();
    let _ = std::thread::Builder::new()
        .name("sonarad-read-log".into())
        .spawn(move || {
            let mut log = ReadLog::new(tags, origins);
            while let Ok(e) = events.recv() {
                for line in log.lines(&e, crate::trace_log::debug()) {
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
            entry: None,
        };
        assert_eq!(session_of(Some("work"), Some(&tag)), "work");
        assert_eq!(session_of(Some(""), Some(&tag)), "c1");
        assert_eq!(session_of(None, None), "direct");
    }

    fn state(id: u64, label: Option<&str>, text: &str, chunks: usize) -> Event {
        state_at(id, label, text, 0, chunks)
    }

    fn state_at(id: u64, label: Option<&str>, text: &str, chunk: usize, chunks: usize) -> Event {
        Event::State(sonara_reader::State {
            seq: 1,
            now_playing: Some(sonara_reader::NowPlaying {
                item_id: ItemId(id),
                label: label.map(str::to_string),
                text: text.into(),
                chunk,
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

    fn tags(id: ItemId) -> Option<Tag> {
        match id.0 {
            2 => Some(Tag {
                channel: "c1".into(),
                host_tab: None,
                announcement: true,
                entry: None,
            }),
            4 | 5 => Some(Tag {
                channel: "c1".into(),
                host_tab: None,
                announcement: false,
                entry: Some(id.0 + 100),
            }),
            _ => None,
        }
    }

    fn events() -> Vec<Event> {
        vec![
            item(1, ItemPhase::Started),
            state_at(1, None, "Secret words.", 0, 2),
            state_at(1, None, "More secret words.", 1, 2),
            item(1, ItemPhase::Finished),
            item(2, ItemPhase::Started),
            state(2, Some("work"), "Session changed: work.", 1),
            item(2, ItemPhase::Skipped),
            // Dropped from the queue unread.
            item(3, ItemPhase::Skipped),
            item(4, ItemPhase::Started),
            state_at(4, Some("work"), "Pick one.", 0, 3),
            state_at(4, Some("work"), "Red.", 1, 3),
            // Skipped by the user before its last chunk.
            item(4, ItemPhase::Skipped),
            item(5, ItemPhase::Skipped),
        ]
    }

    #[test]
    fn one_start_text_and_end_per_item_and_no_text_with_debug_log_off() {
        let origins = Origins::default();
        origins.record(104, "question", "ask question");
        let mut log = ReadLog::new(Box::new(tags), origins);
        let lines: Vec<String> = events().iter().flat_map(|e| log.lines(e, false)).collect();
        assert_eq!(
            lines,
            vec![
                "read start item=1 session=direct chunks=2",
                "read text item=1 session=direct kind=speak from=speak chunks=2/2",
                "read end item=1 finished",
                "read start item=2 session=work chunks=1 kind=announce",
                "read text item=2 session=work kind=announce from=switch chunks=1/1",
                "read end item=2 skipped",
                "read drop item=3 session=direct kind=speak from=speak unread",
                "read start item=4 session=work chunks=3",
                "read text item=4 session=work kind=question from=\"ask question\" chunks=2/3",
                "read end item=4 skipped",
                "read drop item=5 session=c1 kind=speak from=speak unread",
            ]
        );
        assert!(lines.iter().all(|l| !l.contains("ecret")));
    }

    #[test]
    fn an_origin_recorded_after_the_first_state_still_names_the_text() {
        // The channel feeds the reader before the agent records the
        // entry's origin, so the first state of an idle reader can come
        // first (#219 review).
        let origins = Origins::default();
        let mut log = ReadLog::new(Box::new(tags), origins.clone());
        let mut lines = Vec::new();
        lines.extend(log.lines(&item(4, ItemPhase::Started), false));
        lines.extend(log.lines(&state_at(4, Some("work"), "Pick one.", 0, 1), false));
        origins.record(104, "prose", "turn_end");
        lines.extend(log.lines(&item(4, ItemPhase::Finished), false));
        origins.record(105, "question", "ask question");
        lines.extend(log.lines(&item(5, ItemPhase::Skipped), false));
        assert_eq!(
            lines,
            vec![
                "read start item=4 session=work chunks=1",
                "read text item=4 session=work kind=prose from=turn_end chunks=1/1",
                "read end item=4 finished",
                "read drop item=5 session=c1 kind=question from=\"ask question\" unread",
            ]
        );
    }

    #[test]
    fn the_text_line_holds_the_chunks_read_once_each() {
        let mut log = ReadLog::new(Box::new(tags), Origins::default());
        let mut evs = events();
        // A restart of chunk 0 is not read twice in the line.
        evs.insert(3, state_at(1, None, "Secret words.", 0, 2));
        let lines: Vec<String> = evs.iter().flat_map(|e| log.lines(e, true)).collect();
        assert!(
            lines.contains(
                &"read text item=1 session=direct kind=speak from=speak chunks=2/2 \
                  text=\"Secret words. More secret words.\""
                    .to_string()
            ),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("read text item=4")
                && l.ends_with("chunks=2/3 text=\"Pick one. Red.\"")),
            "{lines:#?}"
        );
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
        let mut log = ReadLog::new(Box::new(|_| None), Origins::default());
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
        .flat_map(|e| log.lines(e, true))
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
        let registry = Registry::default();
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
