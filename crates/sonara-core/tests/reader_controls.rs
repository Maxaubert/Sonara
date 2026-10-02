//! Reader: every control in every state (idle, playing, paused, first and
//! last chunk, with and without a queue), mute, volume, rate and voice.
mod common;

use common::{Host, THREE};
use sonara_core::reader::{Control, Effect, ItemPhase};

const ALL: [Control; 10] = [
    Control::Play,
    Control::Pause,
    Control::Toggle,
    Control::Stop,
    Control::Skip,
    Control::Previous,
    Control::Next,
    Control::Restart,
    Control::Mute,
    Control::Unmute,
];

// ---- idle ----

#[test]
fn every_control_except_mute_is_a_no_op_when_idle() {
    for c in ALL {
        if c == Control::Mute {
            continue;
        }
        let mut h = Host::new();
        h.ctl(c);
        assert!(h.last.is_empty(), "{:?} when idle: {:?}", c, h.last);
        assert_eq!(h.state().seq, 0);
    }
}

#[test]
fn mute_when_idle_sets_muted() {
    let mut h = Host::new();
    h.ctl(Control::Mute);
    assert_eq!(h.last[0], Effect::Mute);
    assert!(h.state().muted);
    assert_eq!(h.state().seq, 1);
}

#[test]
fn restart_when_idle_replays_the_last_item() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.play_out();
    h.ctl(Control::Restart);
    assert_eq!(h.item_events_in_last(), vec![(a, ItemPhase::Started)]);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
    h.play_out();
    assert_eq!(h.heard.len(), 6);
}

#[test]
fn restart_when_idle_after_skip_replays_the_skipped_item() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Skip);
    assert!(h.state().now_playing.is_none());
    h.ctl(Control::Restart);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
}

#[test]
fn restart_after_stop_is_a_no_op() {
    let mut h = Host::new();
    h.speak(THREE);
    h.ctl(Control::Stop);
    h.ctl(Control::Restart);
    assert!(h.last.is_empty());
}

#[test]
fn other_controls_after_an_item_ended_stay_no_ops() {
    for c in [
        Control::Previous,
        Control::Next,
        Control::Skip,
        Control::Play,
    ] {
        let mut h = Host::new();
        h.speak(THREE);
        h.play_out();
        h.ctl(c);
        assert!(h.last.is_empty(), "{:?}", c);
    }
}

// ---- playing ----

#[test]
fn pause_then_play_resumes_the_same_chunk() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.finish_chunk();
    h.ctl(Control::Pause);
    assert_eq!(h.last[0], Effect::PauseOutput);
    assert!(h.state().paused);
    h.ctl(Control::Pause);
    assert!(h.last.is_empty(), "second pause is a no-op");
    h.ctl(Control::Play);
    assert_eq!(h.last[0], Effect::ResumeOutput);
    assert!(h.plays_in_last().is_empty());
    assert!(!h.state().paused);
    h.ctl(Control::Play);
    assert!(h.last.is_empty(), "play while playing is a no-op");
    h.play_out();
    assert_eq!(h.heard, vec![(a, 0), (a, 1), (a, 2)]);
}

#[test]
fn toggle_flips_between_paused_and_playing() {
    let mut h = Host::new();
    h.speak(THREE);
    h.ctl(Control::Toggle);
    assert_eq!(h.last[0], Effect::PauseOutput);
    h.ctl(Control::Toggle);
    assert_eq!(h.last[0], Effect::ResumeOutput);
}

#[test]
fn stop_clears_the_current_item_and_the_queue() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee.");
    h.ctl(Control::Stop);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Skipped), (b, ItemPhase::Skipped)]
    );
    let s = h.state();
    assert!(s.now_playing.is_none());
    assert_eq!(s.queued, 0);
    h.ctl(Control::Stop);
    assert!(h.last.is_empty(), "stop twice is a no-op");
}

#[test]
fn stop_while_paused_clears_the_pause() {
    let mut h = Host::new();
    h.speak(THREE);
    h.ctl(Control::Pause);
    h.ctl(Control::Stop);
    assert!(!h.state().paused);
    let b = h.speak("Bee.");
    assert_eq!(
        h.plays_in_last(),
        vec![(b, 0)],
        "a new item plays after stop"
    );
}

#[test]
fn skip_ends_the_current_item_and_starts_the_next() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee one. Bee two.");
    h.ctl(Control::Skip);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Skipped), (b, ItemPhase::Started)]
    );
    assert_eq!(h.plays_in_last(), vec![(b, 0)]);
    assert_eq!(h.state().queued, 0);
}

#[test]
fn skip_with_an_empty_queue_goes_idle() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Skip);
    assert_eq!(h.item_events_in_last(), vec![(a, ItemPhase::Skipped)]);
    assert!(h.state().now_playing.is_none());
}

#[test]
fn next_moves_one_chunk_forward() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Next);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
    assert!(h.item_events_in_last().is_empty());
    assert_eq!(h.state().now_playing.unwrap().chunk, 1);
}

#[test]
fn next_on_the_last_chunk_behaves_like_skip() {
    let mut h = Host::new();
    let a = h.speak("Ay one. Ay two.");
    let b = h.speak("Bee.");
    h.finish_chunk();
    h.ctl(Control::Next);
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Skipped), (b, ItemPhase::Started)]
    );
    assert_eq!(h.plays_in_last(), vec![(b, 0)]);
}

#[test]
fn next_on_the_last_chunk_with_an_empty_queue_goes_idle() {
    let mut h = Host::new();
    h.speak("Only one.");
    h.ctl(Control::Next);
    assert!(h.state().now_playing.is_none());
}

#[test]
fn previous_moves_one_chunk_back() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.finish_chunk();
    h.finish_chunk();
    h.ctl(Control::Previous);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
    assert!(h.synth_in_last().is_empty(), "chunk 1 is still synthesized");
}

#[test]
fn previous_on_the_first_chunk_restarts_it() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Previous);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
}

#[test]
fn restart_goes_back_to_the_first_chunk() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.finish_chunk();
    h.finish_chunk();
    h.ctl(Control::Restart);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
    assert!(
        h.item_events_in_last().is_empty(),
        "same item, no new Started"
    );
    h.play_out();
    assert_eq!(h.heard, vec![(a, 0), (a, 1), (a, 0), (a, 1), (a, 2)]);
}

#[test]
fn restart_on_the_first_chunk_replays_it_once() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Restart);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
}

// ---- paused ----

#[test]
fn navigation_while_paused_moves_but_stays_paused() {
    for c in [Control::Next, Control::Previous, Control::Restart] {
        let mut h = Host::new();
        let a = h.speak(THREE);
        h.finish_chunk();
        h.ctl(Control::Pause);
        h.ctl(c);
        assert_eq!(h.last[0], Effect::StopOutput, "{:?}", c);
        assert!(
            h.plays_in_last().is_empty(),
            "{:?} must not play while paused",
            c
        );
        assert!(h.state().paused);
        let want = if c == Control::Next { 2 } else { 0 };
        assert_eq!(h.state().now_playing.unwrap().chunk, want, "{:?}", c);
        h.ctl(Control::Play);
        assert_eq!(h.plays_in_last(), vec![(a, want)], "{:?}", c);
        // the stopped chunk is gone from the output, so no ResumeOutput
        assert!(!h.last.contains(&Effect::ResumeOutput));
    }
}

#[test]
fn toggle_after_navigation_while_paused_plays_the_new_chunk() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Toggle);
    h.ctl(Control::Next);
    h.ctl(Control::Toggle);
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
}

#[test]
fn skip_while_paused_starts_the_next_item_paused() {
    let mut h = Host::new();
    h.speak(THREE);
    let b = h.speak("Bee.");
    h.ctl(Control::Pause);
    h.ctl(Control::Skip);
    assert!(h.plays_in_last().is_empty());
    assert!(h.state().paused);
    assert_eq!(h.state().now_playing.unwrap().item_id, b);
    h.ctl(Control::Play);
    assert_eq!(h.plays_in_last(), vec![(b, 0)]);
}

#[test]
fn skip_while_paused_with_an_empty_queue_goes_idle_and_unpaused() {
    let mut h = Host::new();
    h.speak(THREE);
    h.ctl(Control::Pause);
    h.ctl(Control::Skip);
    let s = h.state();
    assert!(s.now_playing.is_none());
    assert!(!s.paused);
}

#[test]
fn next_on_the_last_chunk_while_paused_skips_and_stays_paused() {
    let mut h = Host::new();
    h.speak("Only one.");
    let b = h.speak("Bee.");
    h.ctl(Control::Pause);
    h.ctl(Control::Next);
    assert!(h.state().paused);
    assert_eq!(h.state().now_playing.unwrap().item_id, b);
}

#[test]
fn a_paused_reader_ignores_finishing_the_queue_until_play() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Pause);
    h.speak("Bee.");
    h.ctl(Control::Play);
    h.play_out();
    assert_eq!(h.heard[..3], [(a, 0), (a, 1), (a, 2)]);
    assert_eq!(h.heard.len(), 4);
}

// ---- mute, volume, rate, voice ----

#[test]
fn mute_keeps_playback_moving_silently() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Mute);
    assert_eq!(h.last[0], Effect::Mute);
    assert!(h.state().muted);
    assert!(h.output_muted());
    h.ctl(Control::Mute);
    assert!(h.last.is_empty(), "mute twice is a no-op");
    h.finish_chunk();
    assert_eq!(h.plays_in_last(), vec![(a, 1)], "muted still advances");
    h.ctl(Control::Unmute);
    assert_eq!(h.last[0], Effect::Unmute);
    assert!(!h.state().muted);
    h.ctl(Control::Unmute);
    assert!(h.last.is_empty());
}

#[test]
fn mute_survives_stop_and_new_items() {
    let mut h = Host::new();
    h.ctl(Control::Mute);
    h.speak(THREE);
    h.ctl(Control::Stop);
    assert!(h.state().muted);
}

#[test]
fn volume_rate_and_voice_change_the_state_only_on_change() {
    let mut h = Host::new();
    let fx = h.reader.set_volume(60);
    assert_eq!(fx[0], Effect::SetVolume(60));
    assert_eq!(h.reader.state().volume, 60);
    assert!(h.reader.set_volume(60).is_empty());

    let seq = h.reader.state().seq;
    let fx = h.reader.set_rate(250);
    assert_eq!(fx.len(), 1, "rate is read from the state: {:?}", fx);
    assert_eq!(h.reader.state().rate, 250);
    assert_eq!(h.reader.state().seq, seq + 1);
    assert!(h.reader.set_rate(250).is_empty());

    let fx = h.reader.set_voice(Some("af_heart".into()));
    assert_eq!(fx.len(), 1);
    assert_eq!(h.reader.state().voice.as_deref(), Some("af_heart"));
    assert!(h.reader.set_voice(Some("af_heart".into())).is_empty());
    assert_eq!(h.reader.set_voice(None).len(), 1);
}

#[test]
fn defaults_match_the_python_reader() {
    let s = Host::new().state();
    assert_eq!((s.volume, s.rate, s.voice), (100, 200, None));
    assert!(!s.paused && !s.muted);
}
