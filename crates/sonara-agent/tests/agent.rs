//! `Agent` over real `Channels` and a real `ReaderHandle` (fake engine,
//! `TestOutput`): the test decides when each chunk ends. Ported product
//! rules from the Python daemon tests: one message always the last, late
//! text after a new turn dropped, the pause stays on when another channel
//! gets a new turn, decisions read with priority, earcons and mute levels.
use sonara_agent::{
    Agent, Ask, AskKind, Channels, Config, Earcon, Settings, Summarizer, SummarySettings,
};
use sonara_audio::{OutputCall, TestOutput};
use sonara_channels::{Config as ChannelsConfig, Control, Policy};
use sonara_engine::fake::FakeEngine;
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
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
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
