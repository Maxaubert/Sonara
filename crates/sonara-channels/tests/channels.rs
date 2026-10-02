//! `Channels` over a real `ReaderHandle` (fake engine, `TestOutput`): the
//! test decides when each chunk ends, and the helpers wait for the reader
//! and the channels thread with a timeout.
use sonara_audio::TestOutput;
use sonara_channels::{Channels, Config, Control, Policy, QueueMode};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config as ReaderConfig, ReaderHandle, Registry};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Rig {
    ch: Channels,
    out: TestOutput,
}

impl Rig {
    fn new() -> Rig {
        Self::with(Config::default())
    }

    fn with(config: Config) -> Rig {
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(ReaderConfig::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let ch = Channels::new(reader, config).unwrap();
        Rig { ch, out }
    }

    /// Open labelled queue channels `a` ("Alpha") and `b` ("Beta").
    fn two() -> Rig {
        let r = Self::new();
        r.ch.open(
            "a",
            Some("Alpha".into()),
            Some("tab-a".into()),
            Some(Policy::Queue),
        )
        .unwrap();
        r.ch.open("b", Some("Beta".into()), None, Some(Policy::Queue))
            .unwrap();
        r
    }

    fn reader(&self) -> &ReaderHandle {
        self.ch.reader()
    }

    fn playing(&self) -> Option<String> {
        self.reader().state().unwrap().now_playing.map(|n| n.text)
    }

    /// Wait until `text` is loaded in the output.
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

    /// Wait for `text`, then play it to its end.
    fn read(&self, text: &str) {
        self.wait_for(text);
        self.out.start();
        self.out.finish();
    }

    /// Wait until nothing is playing and the channels have nothing to feed.
    fn wait_idle(&self) {
        let end = Instant::now() + TIMEOUT;
        while self.playing().is_some() || !self.ch.is_idle() {
            assert!(Instant::now() < end, "still playing {:?}", self.playing());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Nothing else starts for a moment.
    fn stays_idle(&self) {
        self.wait_idle();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(self.playing(), None);
    }

    fn speak(&self, channel: &str, text: &str) -> Option<u64> {
        self.ch
            .speak(channel, text, None, false, None)
            .unwrap()
            .item_id
            .map(|i| i.0)
    }
}

#[test]
fn entries_are_fed_one_item_at_a_time() {
    let r = Rig::two();
    let first = r.speak("a", "One is here.");
    assert!(first.is_some(), "an idle reader takes the entry at once");
    assert_eq!(r.speak("a", "Two is here."), None, "waits in its channel");
    assert_eq!(r.reader().state().unwrap().queued, 0);
    assert_eq!(r.ch.pending(), 1);
    r.read("One is here.");
    r.read("Two is here.");
    r.stays_idle();
}

#[test]
fn latest_policy_reads_only_the_newest_after_the_current_item() {
    let r = Rig::new();
    r.ch.open("a", None, None, Some(Policy::Latest)).unwrap();
    r.speak("a", "Old one.");
    r.wait_for("Old one.");
    r.speak("a", "Middle one.");
    let s = r.ch.speak("a", "Newest one.", None, false, None).unwrap();
    assert_eq!(s.dropped, 1);
    r.read("Old one.");
    r.read("Newest one.");
    r.stays_idle();
}

#[test]
fn a_mode_overrides_the_policy() {
    let r = Rig::two();
    r.speak("a", "First one.");
    r.speak("a", "Second one.");
    let s =
        r.ch.speak("a", "Third one.", Some(QueueMode::Replace), false, None)
            .unwrap();
    assert_eq!(s.dropped, 1);
    r.read("First one.");
    r.read("Third one.");
    r.stays_idle();
}

#[test]
fn a_switch_is_announced_and_tagged() {
    let r = Rig::two();
    let a = r.speak("a", "From alpha.").unwrap();
    let tag = r.ch.tag(sonara_channels::ItemId(a)).unwrap();
    assert_eq!(tag.channel, "a");
    assert_eq!(tag.host_tab.as_deref(), Some("tab-a"));
    assert!(!tag.announcement);
    r.speak("b", "From beta.");
    r.read("From alpha.");
    r.wait_for("Beta.");
    let id = r.reader().state().unwrap().now_playing.unwrap().item_id;
    let tag = r.ch.tag(id).unwrap();
    assert_eq!(tag.channel, "b");
    assert!(tag.announcement);
    r.read("Beta.");
    r.read("From beta.");
    r.stays_idle();
}

#[test]
fn announcements_can_be_turned_off() {
    let r = Rig::two();
    r.ch.set_announce(false);
    assert!(!r.ch.announce());
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.read("From alpha.");
    r.read("From beta.");
    r.stays_idle();
}

#[test]
fn announcement_text_is_configurable() {
    let r = Rig::with(Config {
        announce_text: "Now {label}.".into(),
        ..Config::default()
    });
    r.ch.open("a", Some("Alpha".into()), None, None).unwrap();
    r.ch.open("b", Some("Beta".into()), None, None).unwrap();
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.read("From alpha.");
    r.read("Now Beta.");
    r.read("From beta.");
}

#[test]
fn next_channel_cuts_announces_and_resumes_the_cut_entry_later() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.wait_for("From alpha.");
    assert_eq!(r.ch.next_channel().unwrap().as_deref(), Some("b"));
    r.read("Beta.");
    r.read("From beta.");
    // Alpha was left on purpose: it is not resumed on its own.
    r.stays_idle();
    assert_eq!(r.ch.next_channel().unwrap().as_deref(), Some("a"));
    // The cut entry was never heard to its end, so it resumes.
    r.read("Alpha.");
    r.read("From alpha.");
    r.stays_idle();
}

#[test]
fn next_channel_replays_a_heard_channel() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.read("From alpha.");
    r.speak("b", "From beta.");
    r.read("Beta.");
    r.read("From beta.");
    assert_eq!(r.ch.next_channel().unwrap().as_deref(), Some("a"));
    r.read("Alpha, reading again.");
    r.read("From alpha.");
    r.stays_idle();
}

#[test]
fn next_channel_without_channels_does_nothing() {
    let r = Rig::new();
    assert_eq!(r.ch.next_channel().unwrap(), None);
}

#[test]
fn closing_the_reading_channel_cuts_it_and_hands_off() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.speak("a", "More alpha.");
    r.speak("b", "From beta.");
    r.wait_for("From alpha.");
    r.ch.close("a").unwrap();
    // The channel that read last is gone: no announcement.
    r.read("From beta.");
    r.stays_idle();
    assert!(r.ch.close("a").is_err());
    assert_eq!(r.ch.channel_ids(), ["b"]);
}

#[test]
fn stop_flushes_every_channel_and_restart_replays_the_engaged_one() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.speak("a", "More alpha.");
    r.speak("b", "From beta.");
    r.wait_for("From alpha.");
    r.ch.control(Control::Stop, None).unwrap();
    r.stays_idle();
    assert_eq!(r.ch.pending(), 0);
    assert_eq!(r.ch.engaged().as_deref(), Some("a"));
    r.ch.control(Control::Restart, None).unwrap();
    r.read("From alpha.");
    r.read("More alpha.");
    r.stays_idle();
}

#[test]
fn stop_with_a_channel_flushes_only_that_channel() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.speak("a", "More alpha.");
    r.speak("b", "From beta.");
    r.wait_for("From alpha.");
    r.ch.control(Control::Stop, Some("b")).unwrap();
    r.read("From alpha.");
    r.read("More alpha.");
    r.stays_idle();
    r.speak("a", "Again alpha.");
    r.wait_for("Again alpha.");
    r.ch.control(Control::Stop, Some("a")).unwrap();
    r.stays_idle();
}

#[test]
fn controls_with_a_channel_apply_only_while_it_reads() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.wait_for("From alpha.");
    r.ch.control(Control::Pause, Some("b")).unwrap();
    assert!(!r.reader().state().unwrap().paused);
    r.ch.control(Control::Pause, Some("a")).unwrap();
    assert!(r.reader().state().unwrap().paused);
    r.ch.control(Control::Play, None).unwrap();
    assert!(!r.reader().state().unwrap().paused);
    assert!(r.ch.control(Control::Pause, Some("zzz")).is_err());
}

#[test]
fn restart_with_a_channel_switches_to_it_and_replays() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.read("From alpha.");
    r.speak("b", "From beta.");
    r.read("Beta.");
    r.wait_for("From beta.");
    r.ch.control(Control::Restart, Some("a")).unwrap();
    r.read("Alpha, reading again.");
    r.read("From alpha.");
    // Beta was cut before its end and is read after.
    r.read("Beta.");
    r.read("From beta.");
    r.stays_idle();
}

#[test]
fn text_spoken_to_the_reader_directly_is_read_first() {
    let r = Rig::two();
    r.reader()
        .speak("Core text.", QueueMode::Append, false, None)
        .unwrap();
    assert_eq!(r.speak("a", "From alpha."), None);
    r.read("Core text.");
    r.read("From alpha.");
    r.stays_idle();
}

#[test]
fn interrupt_reads_the_new_text_now() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.wait_for("From alpha.");
    let s = r.ch.speak("b", "Urgent beta.", None, true, None).unwrap();
    assert_eq!(s.item_id, None, "the announcement goes first");
    r.read("Beta.");
    r.read("Urgent beta.");
    r.stays_idle();
    // Within the reading channel, an interrupt replaces the current item.
    let s = r.ch.speak("b", "Second beta.", None, false, None).unwrap();
    assert!(s.item_id.is_some());
    let s = r.ch.speak("b", "Third beta.", None, true, None).unwrap();
    assert!(s.item_id.is_some());
    r.read("Third beta.");
    r.stays_idle();
}

#[test]
fn focus_picks_the_next_channel_once_the_reader_drains() {
    let r = Rig::two();
    r.ch.open("c", Some("Gamma".into()), None, Some(Policy::Queue))
        .unwrap();
    r.speak("a", "From alpha.");
    r.wait_for("From alpha.");
    r.speak("b", "From beta.");
    r.speak("c", "From gamma.");
    r.ch.focus("c").unwrap();
    assert_eq!(r.ch.focused().as_deref(), Some("c"));
    r.read("From alpha.");
    r.read("Gamma.");
    r.read("From gamma.");
    r.read("Beta.");
    r.read("From beta.");
    r.stays_idle();
    assert!(r.ch.focus("zzz").is_err());
}

#[test]
fn speaking_into_an_unknown_channel_opens_it() {
    let r = Rig::new();
    r.speak("new", "Hello there.");
    let c = r.ch.channel("new").unwrap();
    assert_eq!(c.policy, Policy::Latest);
    assert_eq!(c.label, None);
    r.read("Hello there.");
    assert!(r.ch.speak("", "x", None, false, None).is_err());
}

#[test]
fn the_reader_shutting_down_ends_the_channels() {
    let r = Rig::two();
    r.reader().shutdown();
    assert!(r.ch.speak("a", "Late.", None, false, None).is_err());
}
