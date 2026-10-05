//! #243: muting never loses the session's latest message. On 2026-10-05 a
//! turn ended while super muted and was never stored ("not spoken: mute
//! level 2"); after unmuting, next_channel replayed an older batch with a
//! question that had already been answered. The rules (user decisions of
//! 2026-10-05): every session's latest message is stored whatever the mute
//! level, unmuting reads nothing, a switch to the session or Up reads the
//! stored message, an answered decision is never replayed, and storing
//! sends nothing to the engine and runs no summarizer.
use sonara_agent::earcon::Library;
use sonara_agent::{
    Agent, Ask, AskKind, Channels, Choice, Config, ReadMode, Settings, Summarizer, SummarySettings,
};
use sonara_audio::TestOutput;
use sonara_channels::{Config as ChannelsConfig, Control, Policy};
use sonara_engine::fake::FakeEngine;
use sonara_engine::SendMode;
use sonara_reader::{Config as ReaderConfig, ReaderHandle, Registry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Rig {
    agent: Agent,
    out: TestOutput,
    engine: Arc<FakeEngine>,
    /// The chunk read last: (item, chunk).
    last: Mutex<Option<(sonara_reader::ItemId, usize)>>,
}

impl Rig {
    fn new() -> Rig {
        Self::on(FakeEngine::new(), Settings::default(), None)
    }

    fn on(engine: FakeEngine, settings: Settings, summarizer: Option<Arc<dyn Summarizer>>) -> Rig {
        let engine = Arc::new(engine);
        let registry = Registry::default();
        registry.register(engine.clone()).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(ReaderConfig::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let channels = Channels::new(
            reader,
            ChannelsConfig {
                announce: false,
                ..ChannelsConfig::default()
            },
        )
        .unwrap();
        for (id, label) in [("a", "Alpha"), ("b", "Beta")] {
            channels
                .open(id, Some(label.into()), None, Some(Policy::Latest))
                .unwrap();
        }
        let agent = Agent::new(
            channels,
            Config {
                settings,
                summarizer,
                forget_after: sonara_agent::FORGET_AFTER,
                earcons: Arc::new(Library::bundled()),
            },
        )
        .unwrap();
        Rig {
            agent,
            out,
            engine,
            last: Mutex::new(None),
        }
    }

    fn now(&self) -> Option<sonara_reader::NowPlaying> {
        self.agent.channels().reader().state().unwrap().now_playing
    }

    fn playing(&self) -> Option<String> {
        self.now().map(|n| n.text)
    }

    /// Read the next chunk to its end (never the one read last, whose end
    /// the reader may not have handled yet); returns it with whether it
    /// was its item's last chunk.
    fn read_chunk(&self) -> (String, bool) {
        let end = Instant::now() + TIMEOUT;
        loop {
            if let Some(n) = self.now() {
                let key = (n.item_id, n.chunk);
                let mut last = self.last.lock().unwrap();
                if *last != Some(key) && self.out.loaded().is_some() {
                    *last = Some(key);
                    self.out.start();
                    self.out.finish();
                    return (n.text, n.chunk + 1 >= n.chunks);
                }
            }
            assert!(
                Instant::now() < end,
                "nothing was read: reader {:?}, a {:?}, b {:?}",
                self.agent.channels().reader().state().unwrap(),
                self.agent.channels().channel("a"),
                self.agent.channels().channel("b"),
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Read the next item, every chunk of it; returns its text.
    fn read_next(&self) -> String {
        let mut got = Vec::new();
        loop {
            let (text, last) = self.read_chunk();
            got.push(text);
            if last {
                return got.join(" ");
            }
        }
    }

    fn read(&self, text: &str) {
        assert_eq!(self.read_next(), text);
    }

    fn stays_idle(&self) {
        let end = Instant::now() + TIMEOUT;
        while self.playing().is_some() || !self.agent.channels().is_idle() {
            assert!(Instant::now() < end, "still playing {:?}", self.playing());
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(self.playing(), None);
    }

    /// One whole turn: start, prose (one final block), end.
    fn turn(&self, ch: &str, t: f64, prose: &str) {
        self.agent.turn_start(ch, None, Some(t)).unwrap();
        self.agent
            .stream(ch, None, prose, 0, true, Some(t))
            .unwrap();
        self.agent.turn_end(ch, None, Some(t)).unwrap();
    }

    fn restart(&self) {
        self.agent
            .channels()
            .control(Control::Restart, None)
            .unwrap();
    }
}

fn question(text: &str) -> Ask {
    let mut a = Ask::new(AskKind::Question, text);
    a.options = vec![Choice {
        label: "Yes".into(),
        description: None,
    }];
    a
}

#[test]
fn a_turn_ending_while_super_muted_is_stored_and_read_by_restart_after_unmute() {
    let r = Rig::new();
    r.turn("a", 1.0, "Old reply.");
    r.read("Old reply.");
    r.stays_idle();
    r.agent.set_mute_level(2).unwrap();
    r.turn("a", 2.0, "New reply.");
    r.stays_idle();
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    r.restart();
    r.read("New reply.");
    r.stays_idle();
}

#[test]
fn unmute_reads_nothing() {
    for level in [1, 2] {
        let r = Rig::new();
        r.agent.set_mute_level(level).unwrap();
        r.turn("a", 1.0, "Muted in a.");
        r.turn("b", 1.0, "Muted in b.");
        r.agent.ask("b", &question("Shall I?")).unwrap();
        r.agent.set_mute_level(0).unwrap();
        r.stays_idle();
        assert!(r.engine.texts().is_empty(), "{:?}", r.engine.texts());
        // The latest message of each is stored, unread.
        for ch in ["a", "b"] {
            let c = r.agent.channels().channel(ch).unwrap();
            assert!(!c.entries().is_empty(), "level {level}: {ch} stored");
            assert_eq!(c.pending(), 0, "level {level}: {ch} unread");
        }
    }
}

#[test]
fn next_channel_after_unmute_reads_the_latest_message_not_an_older_one() {
    let r = Rig::new();
    r.turn("a", 1.0, "A old.");
    r.read("A old.");
    r.turn("b", 1.0, "B old.");
    r.read("B old.");
    r.stays_idle();
    r.agent.set_mute_level(2).unwrap();
    r.turn("a", 2.0, "A new.");
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    assert_eq!(
        r.agent.channels().next_channel().unwrap().as_deref(),
        Some("a")
    );
    r.read("A new.");
    r.stays_idle();
}

#[test]
fn a_muted_turn_in_several_chunks_is_stored_whole() {
    let r = Rig::new();
    r.agent.set_mute_level(1).unwrap();
    r.agent.turn_start("a", None, Some(1.0)).unwrap();
    r.agent
        .stream("a", None, "First part. Second part.", 0, true, Some(1.0))
        .unwrap();
    r.agent.tool("a", "Bash", "ls").unwrap();
    r.agent
        .stream("a", None, "Third part.", 0, true, Some(1.0))
        .unwrap();
    r.agent.turn_end("a", None, Some(1.0)).unwrap();
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    assert_eq!(
        r.agent.channels().next_channel().unwrap().as_deref(),
        Some("a")
    );
    for t in ["First part.", "Second part.", "ls", "Third part."] {
        r.read(t);
    }
    r.stays_idle();
}

#[test]
fn an_answered_question_is_not_replayed() {
    let r = Rig::new();
    r.agent.turn_start("a", None, Some(1.0)).unwrap();
    r.agent
        .stream("a", None, "Lead in.", 0, true, Some(1.0))
        .unwrap();
    r.agent.ask("a", &question("Question 9b?")).unwrap();
    r.read("Lead in.");
    assert!(r.read_next().contains("Question 9b"));
    r.stays_idle();
    r.agent.answered("a").unwrap();
    r.agent.turn_end("a", None, Some(1.0)).unwrap();
    r.stays_idle();
    r.restart();
    r.read("Lead in.");
    r.stays_idle();
}

#[test]
fn a_question_answered_while_muted_is_not_replayed() {
    // The log of 2026-10-05: the question was heard, answered, and the
    // rest of the turn came while super muted.
    let r = Rig::new();
    r.agent.turn_start("b", None, Some(1.0)).unwrap();
    r.agent.ask("b", &question("Question 9b?")).unwrap();
    assert!(r.read_next().contains("Question 9b"));
    r.stays_idle();
    r.agent.set_mute_level(2).unwrap();
    r.agent.answered("b").unwrap();
    r.agent
        .stream("b", None, "Done with 9b.", 0, true, Some(1.0))
        .unwrap();
    r.agent.turn_end("b", None, Some(1.0)).unwrap();
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    r.restart();
    r.read("Done with 9b.");
    r.stays_idle();
}

#[test]
fn a_decided_permission_is_not_replayed() {
    let r = Rig::new();
    r.agent.turn_start("a", None, Some(1.0)).unwrap();
    r.agent
        .stream("a", None, "Lead in.", 0, true, Some(1.0))
        .unwrap();
    r.agent
        .ask("a", &Ask::new(AskKind::Permission, "Run the tests"))
        .unwrap();
    r.read("Lead in.");
    assert!(r.read_next().contains("Run the tests"));
    r.stays_idle();
    // Approved: the tool runs (verbosity everything announces it).
    r.agent.tool("a", "Bash", "cargo test").unwrap();
    r.read("cargo test");
    r.agent.turn_end("a", None, Some(1.0)).unwrap();
    r.stays_idle();
    r.restart();
    r.read("Lead in.");
    r.read("cargo test");
    r.stays_idle();
}

#[test]
fn an_unanswered_question_stored_while_muted_is_read_by_a_switch() {
    let r = Rig::new();
    r.turn("b", 1.0, "B old.");
    r.read("B old.");
    r.stays_idle();
    r.agent.set_mute_level(1).unwrap();
    r.agent.turn_start("a", None, Some(2.0)).unwrap();
    r.agent.ask("a", &question("Pick one?")).unwrap();
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    assert_eq!(
        r.agent.channels().next_channel().unwrap().as_deref(),
        Some("a")
    );
    assert!(r.read_next().contains("Pick one"));
    r.stays_idle();
}

struct Counting(AtomicUsize);

impl Summarizer for Counting {
    fn summarize(&self, _text: &str, _s: &SummarySettings) -> Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("A summary.".into())
    }
}

const LONG: &str = "This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold.";

#[test]
fn muted_storage_sends_no_engine_request() {
    for mode in [SendMode::Sentence, SendMode::Message] {
        for read_mode in [ReadMode::Immediate, ReadMode::Queue, ReadMode::Done] {
            for level in [1, 2] {
                let settings = Settings {
                    read_mode,
                    ..Settings::default()
                };
                let r = Rig::on(FakeEngine::with_send_mode(mode), settings, None);
                r.agent.set_mute_level(level).unwrap();
                r.agent.turn_start("a", None, Some(1.0)).unwrap();
                r.agent
                    .stream("a", None, "One. Two.\n\nThree.", 0, true, Some(1.0))
                    .unwrap();
                r.agent.ask("a", &question("Go?")).unwrap();
                r.agent.answered("a").unwrap();
                r.agent.tool("a", "Bash", "ls").unwrap();
                r.agent.turn_end("a", None, Some(1.0)).unwrap();
                r.agent.set_mute_level(0).unwrap();
                r.stays_idle();
                assert_eq!(
                    r.engine.syntheses(),
                    0,
                    "{mode:?} {read_mode:?} level {level}: {:?}",
                    r.engine.texts()
                );
                // The stored message is there for a replay.
                let c = r.agent.channels().channel("a").unwrap();
                assert!(!c.entries().is_empty(), "{mode:?} {read_mode:?}");
            }
        }
    }
}

#[test]
fn summaries_are_not_made_while_muted_the_raw_prose_is_stored() {
    let counting = Arc::new(Counting(AtomicUsize::new(0)));
    let settings = Settings {
        summaries: SummarySettings {
            enabled: true,
            settle_ms: 0,
            model: "test-model".into(),
            ..SummarySettings::default()
        },
        ..Settings::default()
    };
    let r = Rig::on(
        FakeEngine::new(),
        settings,
        Some(counting.clone() as Arc<dyn Summarizer>),
    );
    r.agent.set_mute_level(2).unwrap();
    r.turn("a", 1.0, LONG);
    // The settle window (0 ms) runs on a timer thread.
    let end = Instant::now() + TIMEOUT;
    while r
        .agent
        .channels()
        .channel("a")
        .is_none_or(|c| c.entries().is_empty())
    {
        assert!(Instant::now() < end, "nothing stored");
        std::thread::sleep(Duration::from_millis(2));
    }
    r.agent.set_mute_level(0).unwrap();
    r.stays_idle();
    assert_eq!(counting.0.load(Ordering::SeqCst), 0, "no summarizer run");
    assert_eq!(r.engine.syntheses(), 0);
    r.restart();
    for _ in 0..5 {
        r.read("This filler sentence carries the turn well past the threshold.");
    }
    r.stays_idle();
}
