//! Reader: audio events, stale generations, failures, state seq and rapid
//! control storms (the #128 lesson). The host harness checks the effect
//! contract after every call (no PlayChunk over a loaded chunk, no stuck
//! pause, seq strictly increasing, state only on change).
mod common;

use common::{Host, THREE};
use sonara_core::reader::{AudioEvent, Control, ItemPhase};

// ---- stale events ----

#[test]
fn a_stale_chunk_finished_after_restart_is_ignored() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let old = h.gen().unwrap();
    h.ctl(Control::Restart);
    let seq = h.state().seq;
    h.audio(AudioEvent::ChunkFinished { gen: old });
    assert!(h.last.is_empty(), "{:?}", h.last);
    assert_eq!(h.state().seq, seq);
    assert_eq!(h.playing(), Some((a, 0)));
}

#[test]
fn stale_started_and_failed_events_are_ignored() {
    let mut h = Host::new();
    h.speak(THREE);
    let old = h.gen().unwrap();
    h.ctl(Control::Next);
    h.audio(AudioEvent::ChunkStarted { gen: old });
    assert!(h.last.is_empty());
    h.audio(AudioEvent::Failed {
        gen: old,
        reason: "late".into(),
    });
    assert!(h.last.is_empty());
}

#[test]
fn events_for_a_skipped_or_stopped_item_are_ignored() {
    let mut h = Host::new();
    h.speak(THREE);
    let b = h.speak("Bee one. Bee two.");
    let old = h.gen().unwrap();
    h.ctl(Control::Skip);
    h.audio(AudioEvent::ChunkFinished { gen: old });
    assert!(h.last.is_empty());
    assert_eq!(h.playing(), Some((b, 0)));
    let old = h.gen().unwrap();
    h.ctl(Control::Stop);
    h.audio(AudioEvent::ChunkFinished { gen: old });
    assert!(h.last.is_empty());
}

#[test]
fn an_event_with_an_unknown_future_gen_is_ignored() {
    let mut h = Host::new();
    h.speak(THREE);
    let g = h.gen().unwrap();
    h.audio(AudioEvent::ChunkFinished { gen: g + 100 });
    assert!(h.last.is_empty());
}

#[test]
fn chunk_started_changes_nothing_visible() {
    let mut h = Host::new();
    h.speak(THREE);
    let g = h.gen().unwrap();
    h.audio(AudioEvent::ChunkStarted { gen: g });
    assert!(h.last.is_empty());
}

#[test]
fn a_chunk_that_finishes_while_pausing_advances_but_stays_paused() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Pause);
    h.finish_chunk_racing_pause();
    assert!(h.plays_in_last().is_empty());
    let s = h.state();
    assert!(s.paused);
    assert_eq!(s.now_playing.unwrap().chunk, 1);
    h.ctl(Control::Play);
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
}

#[test]
fn the_last_chunk_finishing_while_pausing_ends_the_item() {
    let mut h = Host::new();
    let a = h.speak("Only one.");
    let b = h.speak("Bee.");
    h.ctl(Control::Pause);
    h.finish_chunk_racing_pause();
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Finished), (b, ItemPhase::Started)]
    );
    assert!(h.state().paused, "the pause carries over to the next item");
    assert!(h.plays_in_last().is_empty());
}

// ---- failures ----

#[test]
fn a_failed_chunk_is_skipped_and_the_item_continues() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.fail_chunk();
    assert!(h.item_events_in_last().is_empty());
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
    h.play_out();
    assert_eq!(h.heard, vec![(a, 1), (a, 2)]);
    assert_eq!(h.items.last(), Some(&(a, ItemPhase::Finished)));
}

#[test]
fn a_failed_last_chunk_after_others_played_finishes_the_item() {
    let mut h = Host::new();
    let a = h.speak("Ay one. Ay two.");
    h.finish_chunk();
    h.fail_chunk();
    assert_eq!(h.item_events_in_last(), vec![(a, ItemPhase::Finished)]);
}

#[test]
fn an_item_whose_every_chunk_fails_is_reported_failed_and_the_queue_moves_on() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee.");
    h.fail_chunk();
    h.fail_chunk();
    h.fail_chunk();
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Failed), (b, ItemPhase::Started)]
    );
    assert_eq!(h.plays_in_last(), vec![(b, 0)]);
}

// ---- state seq ----

#[test]
fn state_seq_increases_by_one_per_change_and_never_repeats_a_state() {
    let mut h = Host::new();
    h.speak(THREE);
    h.speak("Bee.");
    h.ctl(Control::Pause);
    h.ctl(Control::Pause);
    h.ctl(Control::Play);
    h.ctl(Control::Next);
    h.play_out();
    let seqs: Vec<u64> = h.states.iter().map(|s| s.seq).collect();
    let want: Vec<u64> = (1..=seqs.len() as u64).collect();
    assert_eq!(seqs, want);
}

#[test]
fn no_op_calls_emit_no_state() {
    let mut h = Host::new();
    h.speak(THREE);
    let n = h.states.len();
    h.ctl(Control::Play);
    h.ctl(Control::Unmute);
    h.audio(AudioEvent::ChunkFinished { gen: 999 });
    assert_eq!(h.states.len(), n);
}

// ---- storms ----

#[test]
fn rapid_controls_restart_ten_times_plays_chunk_zero_once_each() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.finish_chunk();
    for _ in 0..10 {
        h.ctl(Control::Restart);
        assert_eq!(h.plays_in_last(), vec![(a, 0)]);
    }
    h.play_out();
    assert_eq!(h.heard, vec![(a, 0), (a, 0), (a, 1), (a, 2)]);
}

#[test]
fn rapid_controls_next_previous_storm_never_loses_the_item() {
    let mut h = Host::new();
    let text = "S0 here. S1 here. S2 here. S3 here. S4 here. S5 here.";
    let a = h.speak(text);
    for i in 0..40 {
        h.ctl(if i % 3 == 2 {
            Control::Previous
        } else {
            Control::Next
        });
        if h.state().now_playing.is_none() {
            break;
        }
        assert_eq!(h.plays_in_last().len(), 1);
    }
    // ended by running off the end: the item was skipped, not lost silently
    assert!(h.items.contains(&(a, ItemPhase::Skipped)));
    let b = h.speak(text);
    for _ in 0..20 {
        h.ctl(Control::Previous);
        assert_eq!(h.plays_in_last(), vec![(b, 0)]);
    }
    h.play_out();
    assert_eq!(h.heard.len(), 6);
}

#[test]
fn rapid_controls_pause_toggle_storm_never_sticks() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    for _ in 0..101 {
        h.ctl(Control::Toggle);
    }
    assert!(h.state().paused);
    assert!(h.output_paused());
    for _ in 0..50 {
        h.ctl(Control::Pause);
        h.ctl(Control::Play);
    }
    assert!(!h.state().paused, "pause then play leaves it playing");
    for _ in 0..7 {
        h.ctl(Control::Play);
    }
    assert!(!h.output_paused());
    h.play_out();
    assert_eq!(h.heard, vec![(a, 0), (a, 1), (a, 2)]);
}

#[test]
fn rapid_controls_mixed_storm_keeps_the_contract() {
    // A deterministic pseudo-random walk over every input. The harness checks
    // the contract after each call; at the end the reader must still play
    // whatever is left to the end.
    let controls = [
        Control::Play,
        Control::Pause,
        Control::Toggle,
        Control::Skip,
        Control::Previous,
        Control::Next,
        Control::Restart,
        Control::Mute,
        Control::Unmute,
        Control::Stop,
    ];
    for seed in 1..=20u64 {
        let mut h = Host::new();
        let mut x = seed;
        for _ in 0..400 {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = (x >> 33) as usize;
            match r % 16 {
                0..=9 => h.ctl(controls[r % controls.len()]),
                10 | 11 => {
                    if h.playing().is_some() && !h.output_paused() {
                        h.finish_chunk();
                    }
                }
                12 => {
                    if h.playing().is_some() {
                        h.fail_chunk();
                    }
                }
                13 => {
                    h.speak(THREE);
                }
                14 => {
                    h.speak_with(
                        "Rep one. Rep two.",
                        sonara_core::reader::QueueMode::Replace,
                        false,
                    );
                }
                _ => {
                    h.speak_with("Cut in.", sonara_core::reader::QueueMode::Append, true);
                }
            }
        }
        h.ctl(Control::Play);
        h.play_out();
        let s = h.state();
        assert!(s.now_playing.is_none(), "seed {}: {:?}", seed, s);
        assert_eq!(s.queued, 0);
    }
}
