//! Playback through the handle: the fake engine and `TestOutput`, with the
//! test deciding when audio starts and ends. Every output call and event is
//! asserted in order.
mod common;

use common::{len, play, Rig, THREE};
use sonara_audio::OutputCall;
use sonara_reader::{AudioEvent, Control, ItemId, Key, QueueMode, Value};
use std::time::Duration;

fn speak(r: &Rig, text: &str) -> ItemId {
    r.h.speak(text, QueueMode::Append, false, None).unwrap()
}

#[test]
fn speak_pause_resume_next_restart_skip_stop() {
    let (r, engine) = Rig::new();
    let (one, two, three) = ("One is here.", "Two is here.", "Three is here.");

    // speak: the first chunk plays once synthesized.
    let id = speak(&r, THREE);
    assert_eq!(id, ItemId(1));
    assert_eq!(r.events_now(), ["item 1 Started", "state 1/0"]);
    assert_eq!(r.calls(1), [play(1, 0, 1, len(one, 200))]);
    r.out.start();

    // Pause mid-chunk, then resume the same chunk where it was.
    r.h.control(Control::Pause).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::Pause]);
    assert!(r.out.paused());
    r.h.control(Control::Play).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::Resume]);
    assert_eq!(r.events_now(), ["state 1/0 paused", "state 1/0"]);

    // The chunk ends: the next one plays (it was synthesized ahead).
    r.out.finish();
    assert_eq!(r.calls(1), [play(1, 1, 2, len(two, 200))]);
    assert_eq!(r.events_until("state 1/1"), ["state 1/1"]);

    // Next: cut chunk 1, play chunk 2.
    r.h.control(Control::Next).unwrap();
    assert_eq!(
        r.calls(2),
        [OutputCall::Stop, play(1, 2, 3, len(three, 200))]
    );

    // Restart: back to chunk 0 from the cache, no new synthesis.
    r.h.control(Control::Restart).unwrap();
    assert_eq!(r.calls(2), [OutputCall::Stop, play(1, 0, 4, len(one, 200))]);
    assert_eq!(engine.syntheses(), 3);
    assert_eq!(r.events_now(), ["state 1/2", "state 1/0"]);

    // Previous on the first chunk restarts it.
    r.h.control(Control::Previous).unwrap();
    assert_eq!(r.calls(2), [OutputCall::Stop, play(1, 0, 5, len(one, 200))]);

    // A second item queues behind the first.
    let second = speak(&r, "Second item.");
    assert_eq!(second, ItemId(2));
    assert_eq!(r.events_now(), ["state 1/0 queued 1"]);

    // Skip: the first item ends, the second starts.
    r.h.control(Control::Skip).unwrap();
    assert_eq!(
        r.calls(2),
        [OutputCall::Stop, play(2, 0, 6, len("Second item.", 200))]
    );
    assert_eq!(
        r.events_now(),
        ["item 1 Skipped", "item 2 Started", "state 2/0"]
    );

    // A late event from a superseded play changes nothing.
    r.out.send(AudioEvent::ChunkFinished { gen: 4 });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.calls_now(), []);
    assert_eq!(r.events_now(), Vec::<String>::new());

    // Stop: everything ends.
    r.h.control(Control::Stop).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::Stop]);
    assert_eq!(r.events_now(), ["item 2 Skipped", "state idle"]);
    assert_eq!(r.out.loaded(), None);
    let st = r.h.state().unwrap();
    assert!(st.now_playing.is_none() && st.queued == 0 && !st.paused);
}

#[test]
fn an_item_plays_to_its_end_and_the_next_one_follows() {
    let (r, _) = Rig::new();
    speak(&r, "First one. Second one.");
    speak(&r, "Next item.");
    r.calls(1);
    r.out.finish();
    r.calls(1);
    r.out.finish();
    assert_eq!(r.calls(1), [play(2, 0, 3, len("Next item.", 200))]);
    r.out.finish();
    assert_eq!(
        r.events_until("state idle"),
        [
            "item 1 Started",
            "state 1/0",
            "state 1/0 queued 1",
            "state 1/1 queued 1",
            "item 1 Finished",
            "item 2 Started",
            "state 2/0",
            "item 2 Finished",
            "state idle"
        ]
    );
}

#[test]
fn pause_between_chunks_holds_the_next_chunk_until_play() {
    let (r, _) = Rig::new();
    speak(&r, THREE);
    r.calls(1);
    r.h.control(Control::Pause).unwrap();
    // The chunk was already ending when the pause arrived.
    r.out.finish();
    assert_eq!(
        r.events_until("state 1/1 paused"),
        [
            "item 1 Started",
            "state 1/0",
            "state 1/0 paused",
            "state 1/1 paused"
        ]
    );
    assert_eq!(r.calls_now(), [OutputCall::Pause]);
    r.h.control(Control::Play).unwrap();
    assert_eq!(r.calls(1), [play(1, 1, 2, len("Two is here.", 200))]);
}

#[test]
fn interrupt_and_replace_through_the_handle() {
    let (r, _) = Rig::new();
    speak(&r, "First item.");
    speak(&r, "Queued item.");
    r.calls(1);
    let id =
        r.h.speak(
            "Urgent item.",
            QueueMode::Replace,
            true,
            Some("urgent".into()),
        )
        .unwrap();
    assert_eq!(id, ItemId(3));
    assert_eq!(
        r.calls(2),
        [OutputCall::Stop, play(3, 0, 2, len("Urgent item.", 200))]
    );
    let st = r.h.state().unwrap();
    let np = st.now_playing.unwrap();
    assert_eq!((np.item_id, np.label.as_deref()), (id, Some("urgent")));
    assert_eq!(st.queued, 0);
}

#[test]
fn a_failed_synthesis_skips_its_chunk_and_the_item_continues() {
    let (r, _) = Rig::new();
    speak(&r, "Good one here. Bad [fail] one. Last one here.");
    assert_eq!(r.calls(1), [play(1, 0, 1, len("Good one here.", 200))]);
    r.out.finish();
    // The failed chunk never reaches the output.
    assert_eq!(r.calls(1), [play(1, 2, 3, len("Last one here.", 200))]);
    r.out.finish();
    let events = r.events_until("state idle");
    let logs: Vec<_> = events.iter().filter(|e| e.starts_with("log")).collect();
    assert_eq!(logs.len(), 1, "{events:?}");
    assert!(
        logs[0].starts_with("log synthesis failed for item 1 chunk 1: fake engine failure"),
        "{logs:?}"
    );
    let rest: Vec<_> = events.iter().filter(|e| !e.starts_with("log")).collect();
    assert_eq!(
        rest,
        [
            "item 1 Started",
            "state 1/0",
            "state 1/1",
            "state 1/2",
            "item 1 Finished",
            "state idle"
        ]
    );
}

#[test]
fn a_device_failure_fails_the_item_without_a_panic() {
    let (r, _) = Rig::new();
    r.out.fail_plays(Some("device gone"));
    speak(&r, "Only chunk.");
    assert_eq!(
        r.events_until("state idle"),
        ["item 1 Started", "state 1/0", "item 1 Failed", "state idle"]
    );
    // The reader keeps working once the device is back.
    r.out.fail_plays(None);
    speak(&r, "Again.");
    assert_eq!(r.calls(2)[1..], [play(2, 0, 2, len("Again.", 200))]);
}

#[test]
fn mute_and_volume_reach_the_output() {
    let (r, _) = Rig::new();
    r.h.control(Control::Mute).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::SetVolume(0)]);
    // A new level while muted is kept for unmute, the output stays silent.
    r.h.set(Key::Volume, Value::Number(60)).unwrap();
    assert_eq!(r.calls_now(), []);
    assert_eq!(r.out.volume(), 0);
    r.h.control(Control::Unmute).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::SetVolume(60)]);
    r.h.set(Key::Volume, Value::Number(80)).unwrap();
    assert_eq!(r.calls_now(), [OutputCall::SetVolume(80)]);
    assert_eq!(r.h.get(Key::Volume).unwrap(), Value::Number(80));
    assert_eq!(
        r.events_now(),
        [
            "state idle muted",
            "state idle muted",
            "state idle",
            "state idle"
        ]
    );
}

#[test]
fn rate_reaches_the_engine() {
    let (r, _) = Rig::new();
    r.h.set(Key::Rate, Value::Number(400)).unwrap();
    speak(&r, "Fast words.");
    assert_eq!(r.calls(1), [play(1, 0, 1, len("Fast words.", 400))]);
    assert_eq!(len("Fast words.", 400) * 2, len("Fast words.", 200));
}

#[test]
fn text_with_nothing_to_say_is_skipped() {
    let (r, _) = Rig::new();
    let id = speak(&r, "   ");
    assert_eq!(r.events_now(), [format!("item {} Skipped", id.0)]);
    assert_eq!(r.calls_now(), []);
}

#[test]
fn a_clip_is_mixed_over_the_item_playing_without_touching_it() {
    let (r, _) = Rig::new();
    speak(&r, "One is here.");
    assert_eq!(r.calls(1), [play(1, 0, 1, len("One is here.", 200))]);
    r.out.start();
    r.events_now();
    r.h.play_clip(vec![0; 480], 24_000).unwrap();
    assert_eq!(
        r.calls_now(),
        [OutputCall::PlayClip {
            samples: 480,
            sample_rate: 24_000
        }]
    );
    // No state change, no item event: the item keeps playing.
    assert_eq!(r.events_now(), Vec::<String>::new());
    assert_eq!(r.out.loaded(), Some(1));
}

/// A streaming engine's chunk plays from its first piece (#235): the rest
/// is appended as it comes, and the chunk ends only after its last.
#[test]
fn a_streamed_chunk_plays_as_its_audio_arrives() {
    let (engine, pieces) = common::engines::StreamEngine::new();
    let r = Rig::with(std::sync::Arc::new(engine));
    r.h.set(Key::Engine, Value::Text("stream".into())).unwrap();
    let id = speak(&r, "One long chunk.");
    // Nothing plays before the first audio.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(r.calls_now(), []);
    pieces.send(Some(100)).unwrap();
    assert_eq!(
        r.calls(1),
        [OutputCall::PlayOpen {
            item: id,
            chunk: 0,
            gen: 1,
            samples: 100
        }]
    );
    r.out.start();
    pieces.send(Some(50)).unwrap();
    assert_eq!(
        r.calls(1),
        [OutputCall::Append {
            gen: 1,
            samples: 50
        }]
    );
    pieces.send(None).unwrap();
    assert_eq!(r.calls(1), [OutputCall::Finish { gen: 1 }]);
    r.out.finish();
    assert_eq!(
        r.events_until("item 1 Finished"),
        ["item 1 Started", "state 1/0", "item 1 Finished"]
    );
}

/// Restarting a streamed chunk plays it whole from the cache.
#[test]
fn a_streamed_chunk_replays_whole() {
    let (engine, pieces) = common::engines::StreamEngine::new();
    let r = Rig::with(std::sync::Arc::new(engine));
    r.h.set(Key::Engine, Value::Text("stream".into())).unwrap();
    speak(&r, "One long chunk.");
    pieces.send(Some(10)).unwrap();
    pieces.send(Some(20)).unwrap();
    pieces.send(None).unwrap();
    let calls = r.calls(3);
    assert_eq!(
        calls.last(),
        Some(&OutputCall::Finish { gen: 1 }),
        "{calls:?}"
    );
    r.out.start();
    r.h.control(Control::Restart).unwrap();
    assert_eq!(r.calls(2), [OutputCall::Stop, play(1, 0, 2, 30)]);
}

/// #238: the agent holds the start of a switch announcement until its
/// chime ended. The item is synthesized at once but its first chunk reaches
/// the output only when the hold ends; its next chunk is not held again.
#[test]
fn an_item_does_not_start_before_the_hold_ends() {
    let (r, _) = Rig::new();
    let until = std::time::Instant::now() + Duration::from_millis(300);
    r.h.hold_start(until).unwrap();
    speak(&r, "First one. Second one.");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(r.calls_now(), [], "held: nothing played yet");
    assert_eq!(r.calls(1), [play(1, 0, 1, len("First one.", 200))]);
    let started = r
        .out
        .timed_calls()
        .into_iter()
        .find(|(_, c)| matches!(c, OutputCall::Play { .. }))
        .map(|(at, _)| at)
        .unwrap();
    assert!(started >= until, "played before the hold ended");
    r.out.finish();
    assert_eq!(r.calls(1), [play(1, 1, 2, len("Second one.", 200))]);
}

/// A held item that is stopped meanwhile never plays.
#[test]
fn a_held_item_that_is_stopped_never_plays() {
    let (r, _) = Rig::new();
    r.h.hold_start(std::time::Instant::now() + Duration::from_millis(200))
        .unwrap();
    speak(&r, "Never heard.");
    r.h.control(Control::Stop).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !r.calls_now()
            .iter()
            .any(|c| matches!(c, OutputCall::Play { .. })),
        "a stopped item played"
    );
}
