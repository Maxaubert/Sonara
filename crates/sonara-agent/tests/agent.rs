//! `Agent` over real `Channels` and a real `ReaderHandle` (fake engine,
//! `TestOutput`): the test decides when each chunk ends. Ported product
//! rules from the Python daemon tests: one message always the last, late
//! text after a new turn dropped, the pause stays on when another channel
//! gets a new turn, decisions read with priority, earcons and mute levels.
use sonara_agent::earcon::Library;
use sonara_agent::{
    Agent, Ask, AskKind, BackgroundPolicy, Channels, Config, Earcon, FlushScope, Settings,
    Summarizer, SummarySettings,
};
use sonara_audio::{OutputCall, TestOutput};
use sonara_channels::{Config as ChannelsConfig, Control, Flushed, Policy};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{wav, PcmChunk};
use sonara_reader::{Config as ReaderConfig, ReaderHandle, Registry};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Rig {
    agent: Agent,
    out: TestOutput,
}

impl Rig {
    fn with(settings: Settings, summarizer: Option<Arc<dyn Summarizer>>) -> Rig {
        Self::forgetting(settings, summarizer, sonara_agent::FORGET_AFTER)
    }

    fn forgetting(
        settings: Settings,
        summarizer: Option<Arc<dyn Summarizer>>,
        forget_after: Duration,
    ) -> Rig {
        Self::build(settings, summarizer, forget_after, false, None)
    }

    /// Switch announcements on (the Python plugin's), with `earcons` as
    /// the earcon library (default: the bundled clips).
    fn announcing(earcons: Option<Arc<Library>>) -> Rig {
        Self::build(
            Settings::default(),
            None,
            sonara_agent::FORGET_AFTER,
            true,
            earcons,
        )
    }

    fn build(
        settings: Settings,
        summarizer: Option<Arc<dyn Summarizer>>,
        forget_after: Duration,
        announce: bool,
        earcons: Option<Arc<Library>>,
    ) -> Rig {
        Self::on(
            Arc::new(FakeEngine::new()),
            settings,
            summarizer,
            forget_after,
            announce,
            earcons,
        )
    }

    fn on(
        engine: Arc<FakeEngine>,
        settings: Settings,
        summarizer: Option<Arc<dyn Summarizer>>,
        forget_after: Duration,
        announce: bool,
        earcons: Option<Arc<Library>>,
    ) -> Rig {
        let registry = Registry::default();
        registry.register(engine).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(ReaderConfig::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let channels = Channels::new(
            reader,
            ChannelsConfig {
                announce,
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
                forget_after,
                earcons: earcons.unwrap_or_else(|| Arc::new(Library::bundled())),
            },
        )
        .unwrap();
        Rig { agent, out }
    }

    fn new() -> Rig {
        Self::with(Settings::default(), None)
    }

    fn reader(&self) -> &ReaderHandle {
        self.agent.channels().reader()
    }

    fn playing(&self) -> Option<String> {
        self.reader().state().unwrap().now_playing.map(|n| n.text)
    }

    fn paused(&self) -> bool {
        self.reader().state().unwrap().paused
    }

    fn wait_for(&self, text: &str) {
        let end = Instant::now() + TIMEOUT;
        loop {
            if self.playing().as_deref() == Some(text) && self.out.loaded().is_some() {
                return;
            }
            assert!(
                Instant::now() < end,
                "waited for '{text}', playing {:?}",
                self.playing()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn read(&self, text: &str) {
        self.wait_for(text);
        self.out.start();
        self.out.finish();
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

    fn stream(&self, ch: &str, delta: &str, index: u32, t: Option<f64>) -> bool {
        self.agent.stream(ch, None, delta, index, true, t).unwrap()
    }

    fn clips(&self) -> usize {
        self.out
            .calls()
            .iter()
            .filter(|c| matches!(c, OutputCall::PlayClip { .. }))
            .count()
    }
}

/// The evidence of 2026-10-04 (#235): a reply of 18 sentences in read mode
/// `done` was 18 requests. With an engine that takes whole messages it is
/// one entry, one item, one synthesis; in sentence mode still 18. Up
/// (Restart) replays the whole message.
#[test]
fn a_done_reply_is_one_synthesis_with_an_engine_that_takes_whole_messages() {
    let words = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
    ];
    let sentences: Vec<String> = words.iter().map(|w| format!("This is line {w}.")).collect();
    let reply = format!(
        "{}\n\n{}",
        sentences[..9].join(" "),
        sentences[9..].join(" ")
    );
    for (mode, want) in [
        (sonara_engine::SendMode::Message, 1),
        (sonara_engine::SendMode::Sentence, 18),
    ] {
        let engine = Arc::new(FakeEngine::with_send_mode(mode));
        let settings = Settings {
            read_mode: sonara_agent::ReadMode::Done,
            ..Default::default()
        };
        let r = Rig::on(
            engine.clone(),
            settings,
            None,
            sonara_agent::FORGET_AFTER,
            false,
            None,
        );
        r.agent.turn_start("a", None, Some(1.0)).unwrap();
        for s in reply.split_inclusive('.') {
            r.agent.stream("a", None, s, 0, false, Some(1.0)).unwrap();
        }
        r.agent.stream("a", None, "", 0, true, Some(1.0)).unwrap();
        assert_eq!(engine.syntheses(), 0, "{mode:?}: held until the turn end");
        r.agent.turn_end("a", None, Some(1.0)).unwrap();
        if want == 1 {
            r.read(&reply);
            r.stays_idle();
            assert_eq!(engine.texts(), vec![reply.clone()]);
            // Up replays the whole message (a new item, the same text).
            r.agent.channels().control(Control::Restart, None).unwrap();
            r.read(&reply);
        } else {
            for s in &sentences {
                r.read(s);
            }
        }
        r.stays_idle();
        assert_eq!(
            engine.texts().len(),
            if want == 1 { 2 } else { 18 },
            "{mode:?}: {:?}",
            engine.texts()
        );
        if want == 1 {
            assert_eq!(engine.texts()[1], reply, "the replay is the whole message");
        }
    }
}

#[test]
fn a_turn_is_read_sentence_by_sentence_in_its_channel() {
    let r = Rig::new();
    assert!(r.agent.turn_start("a", None, Some(1.0)).unwrap());
    r.stream("a", "One is here. Two is here.", 0, Some(1.0));
    r.read("One is here.");
    r.read("Two is here.");
    r.stays_idle();
}

#[test]
fn a_new_turn_replaces_what_is_left_of_the_last_one() {
    // One message, always the last: the old turn's item is cut and its
    // unread text dropped; the new turn is read.
    let r = Rig::new();
    r.agent.turn_start("a", None, Some(1.0)).unwrap();
    r.stream("a", "Old one. Old two.", 0, Some(1.0));
    r.wait_for("Old one.");
    r.agent.turn_start("a", None, Some(5.0)).unwrap();
    r.stream("a", "New answer.", 0, Some(5.0));
    r.read("New answer.");
    r.stays_idle();
}

#[test]
fn late_text_from_the_previous_turn_is_never_read() {
    // #174
    let r = Rig::new();
    r.agent.turn_start("a", None, Some(105.0)).unwrap();
    assert!(!r.stream("a", "Old tail.", 1, Some(104.0)));
    assert!(!r.agent.turn_end("a", None, Some(104.5)).unwrap());
    assert_eq!(r.clips(), 0, "the old turn's end plays no chime");
    r.stays_idle();
    assert!(r.stream("a", "New text.", 0, Some(106.0)));
    r.read("New text.");
}

#[test]
fn the_pause_stays_on_when_another_channel_gets_a_new_turn() {
    // upstream #69 (test_background_prompt_keeps_the_foreground_voice_paused
    // and test_own_prompt_still_auto_resumes_the_paused_voice)
    let r = Rig::new();
    r.stream("a", "Alpha speaks.", 0, None);
    r.wait_for("Alpha speaks.");
    r.out.start();
    r.agent.channels().control(Control::Pause, None).unwrap();
    assert!(r.paused());
    r.agent.turn_start("b", None, Some(2.0)).unwrap();
    r.stream("b", "Beta speaks.", 0, Some(2.0));
    assert!(r.paused(), "another channel's turn keeps the pause");
    assert_eq!(r.playing().as_deref(), Some("Alpha speaks."));
    // The engaged channel's own new turn cuts its item and un-pauses; the
    // waiting channel is read, then the new turn (no channel is preferred
    // over another without a decision or focus).
    r.agent.turn_start("a", None, Some(3.0)).unwrap();
    r.stream("a", "Alpha again.", 0, Some(3.0));
    r.wait_for("Beta speaks.");
    assert!(!r.paused());
    r.read("Beta speaks.");
    r.read("Alpha again.");
    r.stays_idle();
}

#[test]
fn a_decision_is_read_before_the_rest_of_another_channels_turn() {
    // test_background_decision_preempts_current_reader
    let r = Rig::new();
    r.stream("a", "One is here. Two is here.", 0, None);
    r.wait_for("One is here.");
    let mut ask = Ask::new(AskKind::Question, "Deploy now?");
    ask.options = vec![sonara_agent::Choice {
        label: "Yes".into(),
        description: None,
    }];
    r.agent.ask("b", &ask).unwrap();
    assert_eq!(r.playing().as_deref(), Some("One is here."), "not cut");
    r.read("One is here.");
    r.read("Deploy now?");
    r.read("Option 1: Yes.");
    r.read("Two is here.");
    r.stays_idle();
}

#[test]
fn earcons_are_mixed_over_speech_and_reported() {
    let r = Rig::new();
    let heard = r.agent.subscribe();
    r.stream("a", "Speaking now.", 0, None);
    r.wait_for("Speaking now.");
    r.out.start();
    r.agent.turn_end("a", None, None).unwrap();
    let clip = Earcon::TurnDone.clip();
    assert!(r.out.calls().contains(&OutputCall::PlayClip {
        samples: clip.samples.len(),
        sample_rate: clip.sample_rate,
    }));
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    assert_eq!(r.playing().as_deref(), Some("Speaking now."), "not cut");
    r.agent.earcon(Earcon::Nav).unwrap();
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::Nav);
}

/// The index of the first clip and of the last `Play` in the output calls.
fn first_clip_and_last_play(out: &TestOutput) -> (usize, usize) {
    let calls = out.calls();
    let clip = calls
        .iter()
        .position(|c| matches!(c, OutputCall::PlayClip { .. }))
        .expect("a clip played");
    let play = calls
        .iter()
        .rposition(|c| matches!(c, OutputCall::Play { .. }))
        .expect("an item played");
    (clip, play)
}

#[test]
fn an_automatic_session_switch_chimes_then_says_session_changed() {
    // Python daemon/__init__.py "Session changed: {0}." and the router's
    // session_change item: the chime first, then the announcement.
    let r = Rig::announcing(None);
    let heard = r.agent.subscribe();
    r.stream("a", "From alpha.", 0, None);
    r.stream("b", "From beta.", 0, None);
    r.read("From alpha.");
    assert!(heard.try_recv().is_err(), "no chime before the switch");
    r.wait_for("Session changed: Beta.");
    let (clip, play) = first_clip_and_last_play(&r.out);
    assert!(clip < play, "the earcon goes before the announcement");
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::SessionChange);
    let c = Earcon::SessionChange.clip();
    assert!(r.out.calls().contains(&OutputCall::PlayClip {
        samples: c.samples.len(),
        sample_rate: c.sample_rate,
    }));
    r.out.start();
    r.out.finish();
    r.read("From beta.");
    r.stays_idle();
    assert!(heard.try_recv().is_err(), "one chime per switch");
}

#[test]
fn a_manual_session_switch_chimes_then_says_session_changed_reading_again() {
    let r = Rig::announcing(None);
    let heard = r.agent.subscribe();
    r.stream("a", "From alpha.", 0, None);
    r.read("From alpha.");
    r.stays_idle();
    r.stream("b", "From beta.", 0, None);
    r.read("Session changed: Beta.");
    r.read("From beta.");
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::SessionChange);
    r.stays_idle();
    let before = r.clips();
    assert_eq!(
        r.agent.channels().next_channel().unwrap().as_deref(),
        Some("a")
    );
    r.wait_for("Session changed: Alpha, reading again.");
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::SessionChange);
    assert_eq!(r.clips(), before + 1);
    r.out.start();
    r.out.finish();
    r.read("From alpha.");
}

#[test]
fn custom_earcons_replace_the_bundled_clips() {
    let dir = std::env::temp_dir().join(format!("sonara-agent-earcons-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, rate) in [("session_change", 8_000), ("turn_done", 12_000)] {
        let pcm = PcmChunk {
            samples: (0..rate / 10)
                .map(|i| if i % 16 < 8 { 9000 } else { -9000 })
                .collect(),
            sample_rate: rate,
            channels: 1,
        };
        std::fs::write(dir.join(format!("{name}.wav")), wav::encode(&pcm)).unwrap();
    }
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let lib = Arc::new(Library::new(dir.clone(), None));
    let r = Rig::announcing(Some(lib));
    r.stream("a", "From alpha.", 0, None);
    r.stream("b", "From beta.", 0, None);
    r.read("From alpha.");
    r.wait_for("Session changed: Beta.");
    assert!(r.out.calls().contains(&OutputCall::PlayClip {
        samples: 800,
        sample_rate: 8_000,
    }));
    // turn_done waits for the session_change clip to end (#238).
    let heard = r.agent.subscribe();
    r.agent.turn_end("b", None, None).unwrap();
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    assert!(r.out.calls().contains(&OutputCall::PlayClip {
        samples: 1200,
        sample_rate: 12_000,
    }));
    assert_eq!(
        r.agent.earcons().custom(),
        [Earcon::TurnDone, Earcon::SessionChange]
    );
}

#[test]
fn the_question_s_permission_prompt_is_suppressed() {
    let r = Rig::new();
    let heard = r.agent.subscribe();
    r.agent
        .ask("a", &Ask::new(AskKind::Question, "Pick?"))
        .unwrap();
    assert!(r.agent.awaiting("a"));
    r.agent
        .ask(
            "a",
            &Ask::new(AskKind::Permission, "Claude needs your permission"),
        )
        .unwrap();
    assert_eq!(heard.try_iter().collect::<Vec<_>>(), [Earcon::Choice]);
    r.read("Pick?");
    r.stays_idle();
}

#[test]
fn mute_levels_silence_speech_then_earcons() {
    let r = Rig::new();
    let heard = r.agent.subscribe();
    r.stream("a", "One is here. Two is here.", 0, None);
    r.wait_for("One is here.");
    r.agent.set_mute_level(1).unwrap();
    r.stays_idle();
    r.stream("a", "Muted text.", 1, None);
    r.agent.turn_end("a", None, None).unwrap();
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    r.stays_idle();
    r.agent.set_mute_level(2).unwrap();
    r.agent.turn_end("a", None, None).unwrap();
    r.agent.earcon(Earcon::Nav).unwrap();
    assert!(heard.try_recv().is_err(), "super muted: no beeps");
    assert!(r.agent.set_mute_level(3).is_err());
    r.agent.set_mute_level(0).unwrap();
    r.agent.turn_start("a", None, None).unwrap();
    r.stream("a", "Back again.", 0, None);
    r.read("Back again.");
}

#[test]
fn an_answer_skips_the_backlog_and_cuts_the_channels_item() {
    let r = Rig::new();
    r.stream("a", "Stale one. Stale two.", 0, None);
    r.wait_for("Stale one.");
    r.agent.answered("a").unwrap();
    r.stays_idle();
    r.stream("a", "After the answer.", 1, None);
    r.read("After the answer.");
}

#[test]
fn closing_forgets_the_channel() {
    let r = Rig::new();
    r.stream("a", "Some text.", 0, None);
    r.wait_for("Some text.");
    r.agent.close("a").unwrap();
    r.stays_idle();
    assert!(r.agent.channels().channel("a").is_none());
    assert!(r.agent.close("").is_err());
}

struct Fake {
    answer: Option<String>,
    seen: Mutex<Vec<String>>,
}

impl Summarizer for Fake {
    fn summarize(&self, text: &str, s: &SummarySettings) -> Result<String, String> {
        self.seen
            .lock()
            .unwrap()
            .push(format!("{}:{text}", s.model));
        self.answer.clone().ok_or_else(|| "no summary".into())
    }
}

fn summary_rig(answer: Option<&str>) -> (Rig, Arc<Fake>) {
    let fake = Arc::new(Fake {
        answer: answer.map(str::to_string),
        seen: Mutex::new(Vec::new()),
    });
    let settings = Settings {
        summaries: SummarySettings {
            enabled: true,
            settle_ms: 0,
            model: "test-model".into(),
            ..SummarySettings::default()
        },
        ..Settings::default()
    };
    let r = Rig::with(settings, Some(fake.clone() as Arc<dyn Summarizer>));
    (r, fake)
}

const LONG: &str = "This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold. \
This filler sentence carries the turn well past the threshold.";

#[test]
fn a_summary_is_read_at_the_end_of_a_long_turn() {
    let (r, fake) = summary_rig(Some("I did the **work**."));
    r.agent.turn_start("a", None, None).unwrap();
    r.stream("a", LONG, 0, None);
    r.stays_idle();
    r.agent.turn_end("a", None, None).unwrap();
    r.read("I did the work.");
    r.stays_idle();
    let seen = fake.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("test-model:This filler"));
}

#[test]
fn a_failed_summary_reads_the_raw_turn() {
    let (r, _) = summary_rig(None);
    r.stream("a", LONG, 0, None);
    r.agent.turn_end("a", None, None).unwrap();
    for _ in 0..5 {
        r.read("This filler sentence carries the turn well past the threshold.");
    }
    r.stays_idle();
}

#[test]
fn summaries_cannot_be_turned_on_without_a_summarizer() {
    let r = Rig::new();
    let on = SummarySettings {
        enabled: true,
        ..SummarySettings::default()
    };
    assert_eq!(
        r.agent.set_summaries(on),
        Err(sonara_agent::Error::NoSummarizer)
    );
    let (r, _) = summary_rig(Some("x"));
    let bad = SummarySettings {
        timeout_s: 5,
        ..SummarySettings::default()
    };
    assert!(r.agent.set_summaries(bad).is_err());
}

// -- background policy (#195), per-channel mute (#196), dead sessions and
// -- earcon subscribers (#197) ------------------------------------------

fn policy(p: BackgroundPolicy) -> Settings {
    Settings {
        background: p,
        ..Settings::default()
    }
}

#[test]
fn earcon_only_reads_the_focused_session_and_chimes_for_the_others() {
    // sessions.py earcon_only (the Python default): a background session's
    // prose and decisions are not read; its earcons play.
    let r = Rig::with(policy(BackgroundPolicy::EarconOnly), None);
    assert!(r.agent.channels().focus_only());
    let earcons = r.agent.subscribe();
    r.agent.channels().focus("a").unwrap();
    r.stream("b", "Background prose.", 0, None);
    r.agent
        .ask("b", &Ask::new(AskKind::Question, "Background question?"))
        .unwrap();
    assert_eq!(earcons.recv_timeout(TIMEOUT).unwrap(), Earcon::Choice);
    r.agent.turn_end("b", None, None).unwrap();
    assert_eq!(earcons.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    r.stays_idle();
    r.stream("a", "Foreground prose.", 0, None);
    r.read("Foreground prose.");
    r.stays_idle();
    // The user prompts the other session: its waiting text is read.
    r.agent.channels().focus("b").unwrap();
    r.read("Background prose.");
    r.read("Background question?");
    r.stays_idle();
}

#[test]
fn policy_all_reads_every_session() {
    let r = Rig::with(policy(BackgroundPolicy::All), None);
    assert!(!r.agent.channels().focus_only());
    r.agent.channels().focus("a").unwrap();
    r.stream("b", "Background prose.", 0, None);
    r.read("Background prose.");
    r.stays_idle();
}

#[test]
fn switching_the_policy_to_all_releases_waiting_text() {
    let r = Rig::new();
    assert_eq!(r.agent.settings().background, BackgroundPolicy::EarconOnly);
    r.agent.channels().focus("a").unwrap();
    r.stream("b", "Was waiting.", 0, None);
    r.stays_idle();
    r.agent
        .set_background_policy(BackgroundPolicy::All)
        .unwrap();
    assert_eq!(r.agent.settings().background, BackgroundPolicy::All);
    r.read("Was waiting.");
    r.stays_idle();
}

#[test]
fn a_background_sessions_summary_is_read_under_earcon_only() {
    // pipeline.py: a digest delivery is authorized past the policy.
    let fake = Arc::new(Fake {
        answer: Some("Background recap.".into()),
        seen: Mutex::new(Vec::new()),
    });
    let settings = Settings {
        background: BackgroundPolicy::EarconOnly,
        summaries: SummarySettings {
            enabled: true,
            settle_ms: 0,
            ..SummarySettings::default()
        },
        ..Settings::default()
    };
    let r = Rig::with(settings, Some(fake as Arc<dyn Summarizer>));
    r.agent.channels().focus("a").unwrap();
    r.stream("b", LONG, 0, None);
    r.agent.turn_end("b", None, None).unwrap();
    r.read("Background recap.");
    r.stays_idle();
}

#[test]
fn a_muted_session_is_not_read_but_still_chimes() {
    // session_prefs muted (router.py): its items wait unread; earcons play.
    let r = Rig::new();
    let earcons = r.agent.subscribe();
    r.agent.set_channel_muted("a", true).unwrap();
    r.stream("a", "Muted text.", 0, None);
    r.agent.turn_end("a", None, None).unwrap();
    assert_eq!(earcons.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    r.stays_idle();
    r.stream("b", "Other session.", 0, None);
    r.read("Other session.");
    r.stays_idle();
    r.agent.set_channel_muted("a", false).unwrap();
    r.read("Muted text.");
    r.stays_idle();
    assert!(r.agent.set_channel_muted("", true).is_err());
}

#[test]
fn muting_the_session_being_read_cuts_it() {
    let r = Rig::new();
    r.stream("a", "One is here. Two is here.", 0, None);
    r.wait_for("One is here.");
    r.agent.set_channel_muted("a", true).unwrap();
    r.stays_idle();
}

#[test]
fn forget_frees_a_dead_sessions_turn_at_once() {
    let r = Rig::new();
    r.agent.turn_start("a", None, Some(10.0)).unwrap();
    assert_eq!(r.agent.tracked(), ["a"]);
    r.agent.forget("a").unwrap();
    assert!(r.agent.tracked().is_empty());
    assert!(r.agent.channels().channel("a").is_none());
    r.agent.forget("never-seen").unwrap();
    // A new session with that id starts clean (no old turn start time).
    assert!(r.stream("a", "Fresh.", 0, Some(1.0)));
    r.read("Fresh.");
}

#[test]
fn a_silent_session_is_forgotten_after_the_timeout() {
    // A session that died without SessionEnd: its turn state is freed and
    // its idle channel closed by a later message (Python forget_session).
    let r = Rig::forgetting(Settings::default(), None, Duration::from_millis(100));
    r.agent.turn_start("a", None, Some(10.0)).unwrap();
    r.agent.turn_start("b", None, Some(10.0)).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    r.agent.turn_start("b", None, Some(11.0)).unwrap();
    assert_eq!(r.agent.tracked(), ["b"], "a was forgotten");
    assert!(r.agent.channels().channel("a").is_none());
    assert!(r.agent.channels().channel("b").is_some());
}

#[test]
fn the_focused_sessions_channel_is_kept_when_its_turn_is_freed() {
    let r = Rig::forgetting(Settings::default(), None, Duration::from_millis(100));
    r.agent.channels().focus("a").unwrap();
    r.agent.turn_start("a", None, None).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    r.agent.turn_start("b", None, None).unwrap();
    assert_eq!(r.agent.tracked(), ["b"]);
    assert!(r.agent.channels().channel("a").is_some(), "focused: kept");
}

#[test]
fn earcon_subscribers_that_went_away_are_pruned() {
    let r = Rig::new();
    let kept = r.agent.subscribe();
    for _ in 0..5 {
        drop(r.agent.subscribe());
    }
    let _last = r.agent.subscribe();
    assert_eq!(r.agent.subscribers(), 2, "pruned at subscription");
    drop(_last);
    r.agent.earcon(Earcon::Nav).unwrap();
    assert_eq!(r.agent.subscribers(), 1, "pruned at the earcon");
    assert_eq!(kept.recv_timeout(TIMEOUT).unwrap(), Earcon::Nav);
}

// -- flush: only the session being read (#228) -----------------------------

fn done() -> Settings {
    Settings {
        read_mode: sonara_agent::ReadMode::Done,
        ..Settings::default()
    }
}

#[test]
fn flush_stops_only_the_session_being_read() {
    let r = Rig::new();
    r.stream("a", "Alpha one. Alpha two.", 0, None);
    r.wait_for("Alpha one.");
    r.out.start();
    r.stream("b", "Beta one.", 0, None);
    assert_eq!(
        r.agent.flush().unwrap().flushed,
        Flushed::Channel("a".into())
    );
    r.read("Beta one.");
    r.stays_idle();
    // The rest of the flushed reply is skipped; its next turn is read.
    r.stream("a", "Alpha later.", 1, None);
    r.stays_idle();
    r.agent.turn_start("a", None, None).unwrap();
    r.stream("a", "Alpha next turn.", 0, None);
    r.read("Alpha next turn.");
}

#[test]
fn flush_while_another_session_streams_keeps_its_message() {
    // #228: read mode `done` holds b's turn while it streams; flushing a
    // must not drop it.
    let r = Rig::with(done(), None);
    r.agent
        .stream("b", None, "Beta still arriving.", 0, false, None)
        .unwrap();
    r.agent
        .ask("a", &Ask::new(AskKind::Plan, "Alpha plan."))
        .unwrap();
    r.wait_for("Plan ready.");
    assert_eq!(
        r.agent.flush().unwrap().flushed,
        Flushed::Channel("a".into())
    );
    r.stays_idle();
    r.agent
        .stream("b", None, " Done now.", 0, true, None)
        .unwrap();
    r.agent.turn_end("b", None, None).unwrap();
    r.read("Beta still arriving.");
    r.read("Done now.");
}

#[test]
fn flush_with_nothing_being_read_drops_nothing() {
    let r = Rig::with(done(), None);
    r.stream("a", "Held for the end.", 0, None);
    assert_eq!(r.agent.flush().unwrap().flushed, Flushed::Nothing);
    r.agent.turn_end("a", None, None).unwrap();
    r.read("Held for the end.");
}

// -- flush_scope all: every ready message (#228) ---------------------------

fn scoped(read_mode: sonara_agent::ReadMode, scope: FlushScope) -> Settings {
    Settings {
        read_mode,
        flush_scope: scope,
        background: BackgroundPolicy::All,
        ..Settings::default()
    }
}

#[test]
fn the_flush_scope_is_session_by_default() {
    assert_eq!(Settings::default().flush_scope, FlushScope::Session);
    let r = Rig::new();
    r.agent.set_flush_scope(FlushScope::All);
    assert_eq!(r.agent.settings().flush_scope, FlushScope::All);
}

#[test]
fn flush_scope_session_reads_the_next_session() {
    let r = Rig::with(
        scoped(sonara_agent::ReadMode::Immediate, FlushScope::Session),
        None,
    );
    r.stream("a", "Alpha one. Alpha two.", 0, None);
    r.wait_for("Alpha one.");
    r.out.start();
    r.agent.turn_end("b", None, None).unwrap();
    r.stream("b", "Beta one.", 0, None);
    let f = r.agent.flush().unwrap();
    assert_eq!(f.flushed, Flushed::Channel("a".into()));
    assert!(f.others.is_empty(), "{:?}", f.others);
    r.read("Beta one.");
    r.stays_idle();
}

#[test]
fn flush_scope_all_drops_every_ready_message() {
    let r = Rig::with(scoped(sonara_agent::ReadMode::Done, FlushScope::All), None);
    r.stream("a", "Alpha one. Alpha two.", 0, None);
    r.agent.turn_end("a", None, None).unwrap();
    r.wait_for("Alpha one.");
    r.out.start();
    // b finished its turn: its whole reply waits behind a.
    r.stream("b", "Beta one. Beta two.", 0, None);
    r.agent.turn_end("b", None, None).unwrap();
    let f = r.agent.flush().unwrap();
    assert_eq!(f.flushed, Flushed::Channel("a".into()));
    assert_eq!(f.others, ["b"]);
    r.stays_idle();
    // b is not skipped for later: its next reply is read.
    r.agent.turn_start("b", None, None).unwrap();
    r.stream("b", "Beta again.", 0, None);
    r.agent.turn_end("b", None, None).unwrap();
    r.read("Beta again.");
}

#[test]
fn flush_while_another_session_streams_keeps_its_message_in_both_scopes() {
    // #228, the incident: another session still writing its reply when the
    // flush lands keeps it, in both scopes and whatever the read mode.
    for scope in [FlushScope::Session, FlushScope::All] {
        for mode in [
            sonara_agent::ReadMode::Immediate,
            sonara_agent::ReadMode::Done,
        ] {
            let r = Rig::with(scoped(mode, scope), None);
            r.agent.turn_start("b", None, None).unwrap();
            r.agent.turn_start("a", None, None).unwrap();
            r.agent
                .ask("a", &Ask::new(AskKind::Plan, "Alpha plan."))
                .unwrap();
            r.wait_for("Plan ready.");
            r.agent
                .stream("b", None, "Beta still arriving.", 0, true, None)
                .unwrap();
            let f = r.agent.flush().unwrap();
            assert_eq!(
                f.flushed,
                Flushed::Channel("a".into()),
                "{scope:?} {mode:?}"
            );
            assert!(f.others.is_empty(), "{scope:?} {mode:?}: {:?}", f.others);
            r.agent
                .stream("b", None, "Done now.", 1, true, None)
                .unwrap();
            r.agent.turn_end("b", None, None).unwrap();
            r.read("Beta still arriving.");
            r.read("Done now.");
            r.stays_idle();
        }
    }
}

#[test]
fn flush_while_another_session_streams_keeps_its_summary_in_both_scopes() {
    // #228, the incident as it happened (summaries on): b still writing
    // when a is flushed gets its summary, made from all of its prose.
    for scope in [FlushScope::Session, FlushScope::All] {
        let fake = Arc::new(Fake {
            answer: Some("Beta summary.".into()),
            seen: Mutex::new(Vec::new()),
        });
        let mut settings = scoped(sonara_agent::ReadMode::Immediate, scope);
        settings.summaries = SummarySettings {
            enabled: true,
            settle_ms: 0,
            model: "test-model".into(),
            ..SummarySettings::default()
        };
        let r = Rig::with(settings, Some(fake.clone() as Arc<dyn Summarizer>));
        r.agent.turn_start("b", None, None).unwrap();
        r.agent.turn_start("a", None, None).unwrap();
        r.agent
            .ask("a", &Ask::new(AskKind::Plan, "Alpha plan."))
            .unwrap();
        r.wait_for("Plan ready.");
        r.agent.stream("b", None, LONG, 0, true, None).unwrap();
        let f = r.agent.flush().unwrap();
        assert_eq!(f.flushed, Flushed::Channel("a".into()), "{scope:?}");
        assert!(f.others.is_empty(), "{scope:?}: {:?}", f.others);
        r.agent
            .stream("b", None, "Beta last words.", 1, true, None)
            .unwrap();
        r.agent.turn_end("b", None, None).unwrap();
        r.read("Beta summary.");
        r.stays_idle();
        let seen = fake.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{scope:?}: {seen:?}");
        assert!(seen[0].contains("This filler"), "{scope:?}: {}", seen[0]);
        assert!(
            seen[0].contains("Beta last words."),
            "{scope:?}: {}",
            seen[0]
        );
    }
}

#[test]
fn flush_scope_all_while_idle_drops_the_ready_messages_it_holds() {
    // Text of a finished turn waiting behind the background policy is
    // ready: an idle flush with scope all drops it too.
    let mut s = scoped(sonara_agent::ReadMode::Immediate, FlushScope::All);
    s.background = BackgroundPolicy::EarconOnly;
    let r = Rig::with(s, None);
    r.agent.channels().focus("a").unwrap();
    r.stream("b", "Background prose.", 0, None);
    r.agent.turn_end("b", None, None).unwrap();
    r.stays_idle();
    let f = r.agent.flush().unwrap();
    assert_eq!(f.flushed, Flushed::Nothing);
    assert_eq!(f.others, ["b"]);
    r.agent.channels().focus("b").unwrap();
    r.stays_idle();
}

#[test]
fn flush_scope_all_skips_the_late_prose_of_a_flushed_finished_reply() {
    // Review of #228: late prose (#14) of another session's reply that a
    // flush-all dropped is not read after the flush.
    let mut s = scoped(sonara_agent::ReadMode::Immediate, FlushScope::All);
    s.background = BackgroundPolicy::EarconOnly;
    let r = Rig::with(s, None);
    r.agent.channels().focus("a").unwrap();
    r.agent.turn_start("b", None, None).unwrap();
    r.stream("b", "Background prose.", 0, None);
    r.agent.turn_end("b", None, None).unwrap();
    let f = r.agent.flush().unwrap();
    assert_eq!(f.others, ["b"]);
    r.stream("b", "Late paragraph.", 1, None);
    r.agent.channels().focus("b").unwrap();
    r.stays_idle();
    // Its next reply is read.
    r.agent.turn_start("b", None, None).unwrap();
    r.stream("b", "Next reply.", 0, None);
    r.read("Next reply.");
}

#[test]
fn a_question_later_in_the_flushed_reply_is_read() {
    let r = Rig::with(
        scoped(sonara_agent::ReadMode::Immediate, FlushScope::Session),
        None,
    );
    r.stream("a", "Alpha one.", 0, None);
    r.wait_for("Alpha one.");
    r.agent.flush().unwrap();
    r.stays_idle();
    r.stream("a", "Skipped lead-in.", 1, None);
    r.agent
        .ask("a", &Ask::new(AskKind::Question, "Deploy now?"))
        .unwrap();
    r.read("Deploy now?");
    r.stays_idle();
}

// -- earcons one after the other (#238) -----------------------------------

/// How long a clip of `samples` at `rate` lasts.
fn clip_len(samples: usize, rate: u32) -> Duration {
    Duration::from_secs_f64(samples as f64 / rate as f64)
}

/// The clips played so far: when, and how many samples at what rate.
fn timed_clips(out: &TestOutput) -> Vec<(Instant, usize, u32)> {
    out.timed_calls()
        .into_iter()
        .filter_map(|(at, c)| match c {
            OutputCall::PlayClip {
                samples,
                sample_rate,
            } => Some((at, samples, sample_rate)),
            _ => None,
        })
        .collect()
}

/// Each clip starts after the one before it ended.
fn assert_no_overlap(clips: &[(Instant, usize, u32)]) {
    for w in clips.windows(2) {
        let (at, samples, rate) = w[0];
        assert!(
            w[1].0 >= at + clip_len(samples, rate),
            "a clip started {:?} after the one before it, which lasts {:?}",
            w[1].0 - at,
            clip_len(samples, rate)
        );
    }
}

/// The evidence of 2026-10-04 (#238): `turn_end` played turn_done and, 1 ms
/// later, the switch to another session played session_change on top of
/// it. Now session_change waits for turn_done to end, and the spoken
/// "Session changed" waits for session_change.
#[test]
fn turn_done_and_session_change_play_one_after_the_other() {
    let r = Rig::announcing(None);
    let heard = r.agent.subscribe();
    r.stream("a", "From alpha.", 0, None);
    r.stream("b", "From beta.", 0, None);
    r.wait_for("From alpha.");
    r.out.start();
    r.agent.turn_end("a", None, None).unwrap();
    r.out.finish();
    r.wait_for("Session changed: Beta.");
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::SessionChange);
    let clips = timed_clips(&r.out);
    let (done, change) = (Earcon::TurnDone.clip(), Earcon::SessionChange.clip());
    assert_eq!(
        clips.iter().map(|c| (c.1, c.2)).collect::<Vec<_>>(),
        [
            (done.samples.len(), done.sample_rate),
            (change.samples.len(), change.sample_rate)
        ],
        "turn_done, then session_change"
    );
    assert_no_overlap(&clips);
    // The announcement is heard after the chime, not under it.
    let spoken = r
        .out
        .timed_calls()
        .into_iter()
        .filter(|(_, c)| matches!(c, OutputCall::Play { .. }))
        .map(|(at, _)| at)
        .next_back()
        .unwrap();
    let (at, samples, rate) = clips[1];
    assert!(
        spoken >= at + clip_len(samples, rate),
        "the announcement started under the session_change chime"
    );
    r.out.start();
    r.out.finish();
    r.read("From beta.");
}

#[test]
fn session_change_is_logged() {
    let r = Rig::announcing(None);
    let traced: Arc<Mutex<Vec<sonara_agent::Trace>>> = Arc::default();
    let t = traced.clone();
    r.agent
        .on_trace(Some(Arc::new(move |x: &sonara_agent::Trace| {
            t.lock().unwrap().push(x.clone())
        })));
    r.stream("a", "From alpha.", 0, None);
    r.stream("b", "From beta.", 0, None);
    r.read("From alpha.");
    r.wait_for("Session changed: Beta.");
    let traced = traced.lock().unwrap();
    assert!(
        traced.iter().any(|t| t.source == "announce"
            && t.channel.as_deref() == Some("b")
            && t.what == sonara_agent::Traced::Earcon(Earcon::SessionChange)),
        "{traced:?}"
    );
}

/// A burst never plays a long chain: an earcon equal to the one queued
/// right before it is dropped, and at most `EARCON_QUEUE` wait or play.
#[test]
fn a_burst_of_earcons_is_capped_and_never_overlaps() {
    let r = Rig::new();
    let heard = r.agent.subscribe();
    let traced: Arc<Mutex<Vec<sonara_agent::Trace>>> = Arc::default();
    let t = traced.clone();
    r.agent
        .on_trace(Some(Arc::new(move |x: &sonara_agent::Trace| {
            t.lock().unwrap().push(x.clone())
        })));
    for e in [
        Earcon::TurnDone,
        Earcon::TurnDone,
        Earcon::Choice,
        Earcon::Permission,
        Earcon::Error,
        Earcon::Nav,
    ] {
        r.agent.earcon(e).unwrap();
    }
    assert_eq!(sonara_agent::EARCON_QUEUE, 3);
    for e in [Earcon::TurnDone, Earcon::Choice, Earcon::Permission] {
        assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), e);
    }
    // Long enough for a fourth to have played after the third.
    assert!(heard.recv_timeout(Duration::from_millis(1500)).is_err());
    assert_eq!(timed_clips(&r.out).len(), 3);
    assert_no_overlap(&timed_clips(&r.out));
    let dropped = traced
        .lock()
        .unwrap()
        .iter()
        .filter(|t| matches!(t.what, sonara_agent::Traced::EarconDropped { .. }))
        .count();
    assert_eq!(dropped, 3, "the duplicate, error and nav are logged");
}

#[test]
fn super_mute_drops_queued_earcons() {
    let r = Rig::new();
    let heard = r.agent.subscribe();
    r.agent.earcon(Earcon::TurnDone).unwrap();
    r.agent.earcon(Earcon::Choice).unwrap();
    assert_eq!(heard.recv_timeout(TIMEOUT).unwrap(), Earcon::TurnDone);
    r.agent.set_mute_level(2).unwrap();
    assert!(heard.recv_timeout(Duration::from_millis(1200)).is_err());
    assert_eq!(timed_clips(&r.out).len(), 1);
}
