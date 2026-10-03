//! `Channels` over a real `ReaderHandle` (fake engine, `TestOutput`): the
//! test decides when each chunk ends, and the helpers wait for the reader
//! and the channels thread with a timeout.
use sonara_audio::OutputCall;
use sonara_audio::TestOutput;
use sonara_channels::{Announced, Channels, Config, Control, Policy, QueueMode};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config as ReaderConfig, ReaderHandle, Registry};
use std::sync::Arc;
use std::sync::Mutex;
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
fn announcement_texts_can_be_changed_on_a_running_driver() {
    let r = Rig::two();
    r.ch.set_announce_texts(
        "Session changed: {label}.",
        "Session changed: {label}, reading again.",
    );
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.read("From alpha.");
    r.read("Session changed: Beta.");
    r.read("From beta.");
    assert_eq!(r.ch.next_channel().unwrap().as_deref(), Some("a"));
    r.read("Session changed: Alpha, reading again.");
    r.read("From alpha.");
}

/// The hook records each announcement and the output calls made so far,
/// so a test can tell whether the hook ran before the announcement was
/// handed to the output.
fn hooked(r: &Rig) -> Arc<Mutex<Vec<(Announced, usize)>>> {
    let seen: Arc<Mutex<Vec<(Announced, usize)>>> = Arc::default();
    let s = seen.clone();
    let out = r.out.clone();
    r.ch.on_announce(Some(Arc::new(move |a: &Announced| {
        let plays = out
            .calls()
            .iter()
            .filter(|c| matches!(c, OutputCall::Play { .. }))
            .count();
        s.lock().unwrap().push((a.clone(), plays));
    })));
    seen
}

fn plays(r: &Rig) -> usize {
    r.out
        .calls()
        .iter()
        .filter(|c| matches!(c, OutputCall::Play { .. }))
        .count()
}

#[test]
fn the_announce_hook_runs_for_automatic_and_manual_switches_before_the_announcement() {
    let r = Rig::two();
    let seen = hooked(&r);
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.read("From alpha.");
    let before = plays(&r);
    r.read("Beta.");
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 1, "the automatic hand-off");
        assert_eq!(s[0].0.channel, "b");
        assert_eq!(s[0].0.label, "Beta");
        assert!(!s[0].0.replay && !s[0].0.manual);
        assert_eq!(
            s[0].1, before,
            "the hook runs before the announcement plays"
        );
    }
    r.read("From beta.");
    let before = plays(&r);
    assert_eq!(r.ch.next_channel().unwrap().as_deref(), Some("a"));
    r.read("Alpha, reading again.");
    let s = seen.lock().unwrap();
    assert_eq!(s.len(), 2, "the manual switch");
    assert_eq!(s[1].0.channel, "a");
    assert!(s[1].0.replay && s[1].0.manual);
    assert_eq!(s[1].1, before);
}

#[test]
fn the_announce_hook_is_quiet_when_announcements_are_off() {
    let r = Rig::two();
    let seen = hooked(&r);
    r.ch.set_announce(false);
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.read("From alpha.");
    r.read("From beta.");
    r.ch.next_channel().unwrap();
    r.read("From alpha.");
    assert!(seen.lock().unwrap().is_empty());
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
    // The cut alpha message was not heard: it follows.
    r.read("Alpha.");
    r.read("From alpha.");
    r.stays_idle();
    // Within the reading channel, an interrupt replaces the current item.
    let s = r.ch.speak("a", "Second alpha.", None, false, None).unwrap();
    assert!(s.item_id.is_some());
    let s = r.ch.speak("a", "Third alpha.", None, true, None).unwrap();
    assert!(s.item_id.is_some());
    r.read("Third alpha.");
    r.stays_idle();
}

#[test]
fn an_interrupt_from_another_channel_reads_the_cut_message_again_after() {
    let r = Rig::new();
    r.ch.open("a", Some("Alpha".into()), None, None).unwrap();
    r.ch.open("b", Some("Beta".into()), None, None).unwrap();
    r.speak("a", "From alpha.");
    r.wait_for("From alpha.");
    r.ch.speak("b", "Urgent beta.", None, true, None).unwrap();
    r.read("Beta.");
    r.read("Urgent beta.");
    // Alpha's only (latest) message was cut, not heard: it is read again.
    r.read("Alpha.");
    r.read("From alpha.");
    r.stays_idle();
}

#[test]
fn next_channel_reads_queued_core_text_between_the_announcement_and_the_message() {
    let r = Rig::two();
    r.speak("a", "From alpha.");
    r.speak("b", "From beta.");
    r.wait_for("From alpha.");
    r.reader()
        .speak("Core text.", QueueMode::Append, false, None)
        .unwrap();
    r.ch.next_channel().unwrap();
    // Documented order: the switch only cuts the current item; text spoken
    // to the reader directly still goes before the channel's next message.
    r.read("Beta.");
    r.read("Core text.");
    r.read("From beta.");
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

#[test]
fn prioritize_reads_a_channel_after_the_current_item_without_cutting_it() {
    let r = Rig::two();
    r.ch.set_announce(false);
    r.speak("a", "One is here.");
    r.speak("a", "Two is here.");
    r.wait_for("One is here.");
    r.speak("b", "A question.");
    r.ch.prioritize("b").unwrap();
    assert_eq!(r.playing().as_deref(), Some("One is here."), "not cut");
    r.read("One is here.");
    r.read("A question.");
    r.read("Two is here.");
    r.stays_idle();
    assert!(matches!(
        r.ch.prioritize("zz"),
        Err(sonara_channels::Error::UnknownChannel(_))
    ));
}

#[test]
fn muting_the_channel_being_read_cuts_it_and_unmuting_reads_the_rest() {
    let r = Rig::two();
    r.ch.set_announce(false);
    r.speak("a", "One is here.");
    r.speak("a", "Two is here.");
    r.wait_for("One is here.");
    r.ch.set_muted("a", true).unwrap();
    assert!(r.ch.is_muted("a"));
    r.wait_idle();
    r.speak("b", "From beta.");
    r.read("From beta.");
    r.stays_idle();
    r.ch.set_muted("a", false).unwrap();
    r.read("Two is here.");
    r.stays_idle();
}

#[test]
fn focus_only_holds_added_text_but_not_a_hosts_speak() {
    let r = Rig::two();
    r.ch.set_announce(false);
    r.ch.focus("a").unwrap();
    r.ch.set_focus_only(true).unwrap();
    assert!(r.ch.focus_only());
    r.ch.add("b", "Agent prose.").unwrap();
    r.stays_idle();
    r.speak("b", "Host text.");
    r.read("Agent prose.");
    r.read("Host text.");
    r.stays_idle();
    r.ch.add("b", "Held again.").unwrap();
    r.stays_idle();
    assert!(r.ch.authorize("b").unwrap());
    r.read("Held again.");
    // Once its batch is read, the channel is held back again.
    r.wait_idle();
    r.ch.add("b", "And again.").unwrap();
    r.stays_idle();
    r.ch.set_focus_only(false).unwrap();
    r.read("And again.");
    r.stays_idle();
}

/// One drop reported (#219): (channel, text, reason, cut while read).
type Reported = (String, String, String, bool);

/// Every drop reported.
fn drops(r: &Rig) -> Arc<Mutex<Vec<Reported>>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    r.ch.on_drop(Some(Arc::new(move |d: &sonara_channels::Dropped| {
        sink.lock().unwrap().push((
            d.channel.clone(),
            d.text.clone(),
            d.reason.clone(),
            d.item.is_some(),
        ));
    })));
    got
}

#[test]
fn text_dropped_unread_is_reported_with_its_reason() {
    let r = Rig::two();
    r.ch.set_announce(false);
    let got = drops(&r);
    let first = r.ch.speak("a", "First one.", None, false, None).unwrap();
    assert!(first.item_id.is_some());
    r.wait_for("First one.");
    r.ch.speak("a", "Second one.", None, false, None).unwrap();
    r.ch.speak("a", "Third one.", Some(QueueMode::Replace), false, None)
        .unwrap();
    assert_eq!(
        got.lock().unwrap().clone(),
        vec![(
            "a".to_string(),
            "Second one.".to_string(),
            "replaced by newer text (mode replace)".to_string(),
            false
        )]
    );
    got.lock().unwrap().clear();
    r.ch.add("b", "Bee text.").unwrap();
    // A new turn on `a`: its unread entry and the item being read go.
    r.ch.control_because(Control::Stop, Some("a"), "turn_start")
        .unwrap();
    let mut seen = got.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("a".into(), "First one.".into(), "turn_start".into(), true),
            ("a".into(), "Third one.".into(), "turn_start".into(), false),
        ]
    );
    got.lock().unwrap().clear();
    r.wait_for("Bee text.");
    r.ch.control_because(Control::Stop, None, "mute").unwrap();
    assert_eq!(
        got.lock().unwrap().clone(),
        vec![("b".into(), "Bee text.".into(), "mute".into(), true)]
    );
}

#[test]
fn spoken_text_tells_its_entry_and_the_tag_carries_it() {
    let r = Rig::two();
    let s = r.ch.speak("a", "Tagged.", None, false, None).unwrap();
    let id = s.item_id.expect("fed at once");
    let tag = r.ch.tag(id).unwrap();
    assert_eq!(tag.entry, Some(s.entry));
    assert!(!tag.announcement);
}
