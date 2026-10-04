//! Reader: prefetch depth (`set_lookahead`, spec D10): a cloud engine asks
//! for more chunks ahead of the playing one, in play order across items.
mod common;

use common::{Host, THREE};
use sonara_core::reader::{Control, ItemId, QueueMode};

fn synth(h: &Host) -> Vec<(ItemId, usize)> {
    h.synth_in_last().iter().map(|s| (s.0, s.1)).collect()
}

#[test]
fn lookahead_two_requests_two_ahead() {
    let mut h = Host::new();
    h.lookahead(2);
    let a = h.speak(THREE);
    assert_eq!(synth(&h), vec![(a, 0), (a, 1), (a, 2)]);
    h.finish_chunk();
    assert!(synth(&h).is_empty(), "everything was already requested");
}

#[test]
fn lookahead_crosses_item_boundary() {
    let mut h = Host::new();
    h.lookahead(2);
    let a = h.speak("Ay one. Ay two.");
    assert_eq!(synth(&h), vec![(a, 0), (a, 1)]);
    let b = h.speak("Bee one. Bee two. Bee three.");
    assert_eq!(synth(&h), vec![(b, 0)], "a/1 and b/0 are the two ahead");
    h.finish_chunk();
    assert_eq!(synth(&h), vec![(b, 1)]);
    h.finish_chunk();
    assert_eq!(h.playing(), Some((b, 0)));
    assert_eq!(synth(&h), vec![(b, 2)]);
}

#[test]
fn lookahead_change_while_playing_requests_more() {
    let mut h = Host::new();
    let a = h.speak("One. Two. Three. Four. Five.");
    assert_eq!(synth(&h), vec![(a, 0), (a, 1)]);
    h.lookahead(3);
    assert_eq!(synth(&h), vec![(a, 2), (a, 3)]);
    h.lookahead(1);
    assert!(synth(&h).is_empty(), "a smaller depth requests nothing");
    h.lookahead(9);
    assert_eq!(
        synth(&h),
        vec![(a, 4)],
        "clamped to 4, and only what is left"
    );
}

#[test]
fn lookahead_applies_while_paused_on_a_loaded_chunk() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Pause);
    h.lookahead(3);
    assert_eq!(synth(&h), vec![(a, 2)], "the paused chunk is still loaded");
    h.ctl(Control::Play);
    assert!(synth(&h).is_empty());
}

#[test]
fn rapid_controls_with_lookahead_three() {
    // No duplicate Synthesize and none for an ended item: the Host checks
    // both on every call.
    let mut h = Host::new();
    h.lookahead(3);
    let controls = [
        Control::Next,
        Control::Skip,
        Control::Previous,
        Control::Pause,
        Control::Next,
        Control::Play,
        Control::Restart,
        Control::Stop,
        Control::Restart,
        Control::Toggle,
    ];
    for round in 0..40usize {
        let text = format!("R{round} one. R{round} two. R{round} three. R{round} four.");
        let mode = if round % 3 == 0 {
            QueueMode::Replace
        } else {
            QueueMode::Append
        };
        h.speak_with(&text, mode, round % 5 == 0);
        h.ctl(controls[round % controls.len()]);
        if round % 4 == 1 {
            h.lookahead(1 + round % 4);
        }
        if h.playing().is_some() && !h.output_paused() {
            h.finish_chunk();
        }
    }
    h.ctl(Control::Play);
    h.play_out();
}
