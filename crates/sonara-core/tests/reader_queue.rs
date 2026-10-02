//! Reader: speak, chunking, queue modes, interrupt, prefetch and item events.
mod common;

use common::{Host, THREE};
use sonara_core::reader::{
    split_chunks, Control, Effect, Event, ItemId, ItemPhase, QueueMode, Reader,
};

#[test]
fn speak_when_idle_starts_the_item_and_plays_its_first_chunk() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    assert_eq!(a, ItemId(1));
    assert_eq!(h.item_events_in_last(), vec![(a, ItemPhase::Started)]);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
    let synth: Vec<usize> = h.synth_in_last().iter().map(|s| s.1).collect();
    assert_eq!(synth, vec![0, 1], "current chunk plus one ahead");
    let s = h.state();
    let np = s.now_playing.expect("now playing");
    assert_eq!((np.item_id, np.chunk, np.chunks), (a, 0, 3));
    assert_eq!(np.text, "One is here.");
    assert_eq!(s.seq, 1);
    assert!(!s.paused);
    assert_eq!(h.emitted_states(), 1);
}

#[test]
fn effects_come_in_order_synthesize_before_play() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let first_synth = h
        .last
        .iter()
        .position(|e| matches!(e, Effect::Synthesize { chunk: 0, .. }))
        .unwrap();
    let play = h
        .last
        .iter()
        .position(|e| matches!(e, Effect::PlayChunk { .. }))
        .unwrap();
    assert!(first_synth < play);
    // the state event is the last effect of the call
    assert!(matches!(h.last.last(), Some(Effect::Emit(Event::State(_)))));
    let _ = a;
}

#[test]
fn ids_increase_and_never_repeat() {
    let mut h = Host::new();
    let ids: Vec<ItemId> = (0..5).map(|_| h.speak(THREE)).collect();
    assert_eq!(ids, (1..=5).map(ItemId).collect::<Vec<_>>());
}

#[test]
fn chunks_are_the_spoken_sentences_and_paragraphs_do_not_split() {
    let chunks = split_chunks("First one here.\n\nSecond **bold** one.\n\n\nThird one.");
    assert_eq!(
        chunks,
        vec!["First one here.", "Second bold one.", "Third one."]
    );
}

#[test]
fn synthesize_carries_the_cleaned_chunk_text() {
    let mut h = Host::new();
    let a = h.speak("Run `cargo test` now. Then **ship** it.");
    let synth = h.synth_in_last();
    let expected = split_chunks("Run `cargo test` now. Then **ship** it.");
    assert_eq!(synth[0], (a, 0, expected[0].clone()));
    assert_eq!(synth[1], (a, 1, expected[1].clone()));
    assert_eq!(h.reader.current().unwrap().chunks, expected);
}

#[test]
fn append_queues_behind_the_current_item_and_plays_in_order() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee one. Bee two.");
    assert!(
        h.plays_in_last().is_empty(),
        "append must not cut the current item"
    );
    assert_eq!(h.state().queued, 1);
    h.play_out();
    assert_eq!(
        h.heard,
        vec![(a, 0), (a, 1), (a, 2), (b, 0), (b, 1)],
        "every chunk once, in order"
    );
    assert_eq!(
        h.items,
        vec![
            (a, ItemPhase::Started),
            (a, ItemPhase::Finished),
            (b, ItemPhase::Started),
            (b, ItemPhase::Finished)
        ]
    );
    let s = h.state();
    assert!(s.now_playing.is_none());
    assert_eq!(s.queued, 0);
}

#[test]
fn replace_drops_unread_items_but_keeps_the_current_one() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee.");
    let c = h.speak("Sea.");
    let d = h.speak_with("Dee.", QueueMode::Replace, false);
    assert_eq!(
        h.item_events_in_last(),
        vec![(b, ItemPhase::Skipped), (c, ItemPhase::Skipped)]
    );
    assert!(!h.last.contains(&Effect::StopOutput));
    assert_eq!(h.state().queued, 1);
    assert_eq!(h.state().now_playing.unwrap().item_id, a);
    h.play_out();
    assert_eq!(h.heard, vec![(a, 0), (a, 1), (a, 2), (d, 0)]);
}

#[test]
fn replace_when_idle_just_plays() {
    let mut h = Host::new();
    let a = h.speak_with(THREE, QueueMode::Replace, false);
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
}

#[test]
fn interrupt_cuts_the_current_item_and_plays_the_new_one_first() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee.");
    let d = h.speak_with("Dee one. Dee two.", QueueMode::Append, true);
    assert_eq!(h.last[0], Effect::StopOutput);
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Skipped), (d, ItemPhase::Started)]
    );
    assert_eq!(h.plays_in_last(), vec![(d, 0)]);
    assert_eq!(
        h.state().queued,
        1,
        "append keeps the queue behind the new item"
    );
    h.play_out();
    assert_eq!(h.heard, vec![(d, 0), (d, 1), (b, 0)]);
}

#[test]
fn interrupt_with_replace_drops_everything_else() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    let b = h.speak("Bee.");
    let d = h.speak_with("Dee.", QueueMode::Replace, true);
    assert_eq!(
        h.item_events_in_last(),
        vec![
            (b, ItemPhase::Skipped),
            (a, ItemPhase::Skipped),
            (d, ItemPhase::Started)
        ]
    );
    assert_eq!(h.state().queued, 0);
    h.play_out();
    assert_eq!(h.heard, vec![(d, 0)]);
}

#[test]
fn interrupt_when_idle_just_plays() {
    let mut h = Host::new();
    let a = h.speak_with(THREE, QueueMode::Append, true);
    assert!(!h.last.contains(&Effect::StopOutput));
    assert_eq!(h.plays_in_last(), vec![(a, 0)]);
}

#[test]
fn interrupt_while_paused_replaces_the_item_but_keeps_the_pause() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Pause);
    let d = h.speak_with("Dee.", QueueMode::Append, true);
    assert_eq!(
        h.item_events_in_last(),
        vec![(a, ItemPhase::Skipped), (d, ItemPhase::Started)]
    );
    assert!(h.plays_in_last().is_empty());
    let s = h.state();
    assert!(s.paused);
    assert_eq!(s.now_playing.unwrap().item_id, d);
    h.ctl(Control::Play);
    assert_eq!(h.plays_in_last(), vec![(d, 0)]);
}

#[test]
fn speak_while_paused_queues_without_playing() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.ctl(Control::Pause);
    h.speak("Bee.");
    assert!(h.plays_in_last().is_empty());
    assert!(h.state().paused);
    assert_eq!(h.state().now_playing.unwrap().item_id, a);
}

#[test]
fn empty_or_unspeakable_text_is_a_no_op_reported_as_skipped() {
    for text in ["", "   \n\t  ", "\n\n\n", "*** --- ***"] {
        let mut h = Host::new();
        let a = h.speak(THREE);
        let seq = h.state().seq;
        let e = h.speak_with(text, QueueMode::Replace, true);
        assert_ne!(e, a, "a fresh id even for a no-op");
        assert_eq!(
            h.last,
            vec![Effect::Emit(Event::Item {
                item_id: e,
                phase: ItemPhase::Skipped
            })],
            "text {:?} must change nothing",
            text
        );
        assert_eq!(h.state().seq, seq);
        assert_eq!(h.state().now_playing.unwrap().item_id, a);
    }
}

#[test]
fn empty_text_when_idle_starts_nothing() {
    let mut h = Host::new();
    h.speak("   ");
    assert!(h.state().now_playing.is_none());
    assert_eq!(h.state().seq, 0);
}

#[test]
fn unicode_text_is_chunked_without_panics() {
    let text = "Caf\u{e9} \u{2615} is open. \u{65e5}\u{672c}\u{8a9e} text here. Go \u{2192} now! \u{1f600} Fin.";
    let mut h = Host::new();
    let a = h.speak(text);
    let chunks = split_chunks(text);
    assert!(chunks.len() >= 3, "{:?}", chunks);
    assert_eq!(h.state().now_playing.unwrap().chunks, chunks.len());
    h.play_out();
    assert_eq!(h.heard.len(), chunks.len());
    assert!(h.heard.iter().all(|(i, _)| *i == a));
}

#[test]
fn label_is_shown_in_the_state() {
    let mut r = Reader::new();
    r.speak(THREE, QueueMode::Append, false, Some("Tab 2".into()));
    assert_eq!(
        r.state().now_playing.unwrap().label.as_deref(),
        Some("Tab 2")
    );
}

#[test]
fn prefetch_stays_one_chunk_ahead() {
    let mut h = Host::new();
    let a = h.speak(THREE);
    h.finish_chunk();
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
    let synth: Vec<usize> = h.synth_in_last().iter().map(|s| s.1).collect();
    assert_eq!(synth, vec![2]);
    h.finish_chunk();
    assert_eq!(h.plays_in_last(), vec![(a, 2)]);
    assert!(h.synth_in_last().is_empty(), "nothing left to prefetch");
}

#[test]
fn prefetch_reaches_into_the_next_item_on_the_last_chunk() {
    let mut h = Host::new();
    let a = h.speak("Ay one. Ay two.");
    let b = h.speak("Bee one. Bee two.");
    h.finish_chunk();
    assert_eq!(h.plays_in_last(), vec![(a, 1)]);
    assert_eq!(h.synth_in_last()[0].0, b);
    assert_eq!(h.synth_in_last()[0].1, 0);
    h.finish_chunk();
    assert_eq!(h.plays_in_last(), vec![(b, 0)]);
    let synth: Vec<(ItemId, usize)> = h.synth_in_last().iter().map(|s| (s.0, s.1)).collect();
    assert_eq!(synth, vec![(b, 1)], "b/0 was already prefetched");
}

#[test]
fn speak_during_the_last_chunk_prefetches_the_new_item() {
    let mut h = Host::new();
    let _a = h.speak("Only one.");
    let b = h.speak("Bee one. Bee two.");
    let synth: Vec<(ItemId, usize)> = h.synth_in_last().iter().map(|s| (s.0, s.1)).collect();
    assert_eq!(synth, vec![(b, 0)]);
}

#[test]
fn a_finished_item_reports_finished_and_the_reader_goes_idle() {
    let mut h = Host::new();
    let a = h.speak("Only one.");
    h.finish_chunk();
    assert_eq!(h.item_events_in_last(), vec![(a, ItemPhase::Finished)]);
    assert!(h.plays_in_last().is_empty());
    assert!(h.state().now_playing.is_none());
    assert_eq!(h.emitted_states(), 1);
}
