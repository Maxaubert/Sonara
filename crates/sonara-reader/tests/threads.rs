//! The handle across threads: concurrent calls, subscriber fan-out, slow
//! engines off the control path, engine errors, shutdown, control storms.
mod common;

use common::engines::{BrokenEngine, GateEngine, SlowWarmEngine};
use common::{fmt, len, play, Rig, THREE, TIMEOUT};
use sonara_audio::OutputCall;
use sonara_reader::{
    AudioEvent, Control, Error, Event, ItemId, ItemPhase, Key, QueueMode, ReaderHandle, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

fn speak(h: &ReaderHandle, text: &str) -> ItemId {
    h.speak(text, QueueMode::Append, false, None).unwrap()
}

#[test]
fn the_handle_is_clone_send_and_sync() {
    fn check<T: Clone + Send + Sync + 'static>() {}
    check::<ReaderHandle>();
}

#[test]
fn concurrent_speaks_get_unique_ids_and_all_queue() {
    let (r, _) = Rig::new();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let h = r.h.clone();
            thread::spawn(move || {
                (0..25)
                    .map(|i| speak(&h, &format!("Thread {t} item {i}.")))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let ids: BTreeSet<_> = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect();
    assert_eq!(ids, (1..=200).map(ItemId).collect());
    let st = r.h.state().unwrap();
    assert_eq!(st.now_playing.unwrap().item_id, ItemId(1));
    assert_eq!(st.queued, 199);
}

#[test]
fn every_subscriber_gets_every_event_and_a_dropped_one_is_forgotten() {
    let (r, _) = Rig::new();
    let a = r.h.subscribe().unwrap();
    let b = r.h.subscribe().unwrap();
    let dropped = r.h.subscribe().unwrap();
    drop(dropped);
    speak(&r.h, "Hello there.");
    r.calls(1);
    r.out.finish();
    let main = r.events_until("state idle");
    let other: Vec<_> = a.try_iter().map(|e| fmt(&e)).collect();
    assert_eq!(other, main);
    let other: Vec<_> = b.try_iter().map(|e| fmt(&e)).collect();
    assert_eq!(other, main);
    // A subscriber added later sees only what follows.
    let late = r.h.subscribe().unwrap();
    r.h.control(Control::Mute).unwrap();
    let got: Vec<_> = late.try_iter().map(|e| fmt(&e)).collect();
    assert_eq!(got, ["state idle muted"]);
}

#[test]
fn a_subscriber_that_never_reads_does_not_block_the_reader() {
    let (r, _) = Rig::new();
    let _idle = r.h.subscribe().unwrap();
    let t0 = Instant::now();
    for i in 0..2_000 {
        speak(&r.h, &format!("Item {i}."));
    }
    r.h.control(Control::Stop).unwrap();
    assert!(t0.elapsed() < TIMEOUT, "took {:?}", t0.elapsed());
    assert!(r.h.state().unwrap().now_playing.is_none());
}

#[test]
fn controls_do_not_wait_for_a_slow_synthesis() {
    let engine = Arc::new(GateEngine::default());
    let r = Rig::with(engine.clone());
    speak(&r.h, THREE);
    engine.wait_started(1);
    // The engine is stuck; controls still answer and the state follows.
    r.h.control(Control::Pause).unwrap();
    assert!(r.h.state().unwrap().paused);
    assert_eq!(r.calls_now(), []);
    // The audio arrives while paused: it waits for Play.
    engine.open();
    engine.wait_started(2);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(r.calls_now(), []);
    r.h.control(Control::Play).unwrap();
    assert_eq!(r.calls_now(), [play(1, 0, 1, len("One is here.", 200))]);
}

#[test]
fn stop_cancels_the_synthesis_in_flight() {
    let engine = Arc::new(GateEngine::default());
    let r = Rig::with(engine.clone());
    speak(&r.h, "Never heard.");
    engine.wait_started(1);
    r.h.control(Control::Stop).unwrap();
    assert_eq!(engine.cancels(), 1);
    assert_eq!(
        r.events_now(),
        [
            "item 1 Started",
            "state 1/0",
            "item 1 Skipped",
            "state idle"
        ]
    );
    // The cancelled result is dropped; the next item speaks normally.
    engine.open();
    speak(&r.h, "Heard.");
    assert_eq!(r.calls(1), [play(2, 0, 2, len("Heard.", 200))]);
    assert_eq!(r.calls_now(), []);
}

#[test]
fn an_engine_that_cannot_speak_fails_items_and_says_why() {
    let r = Rig::with(Arc::new(BrokenEngine));
    speak(&r.h, "First. Second.");
    let events = r.events_until("state idle");
    let (logs, rest): (Vec<_>, Vec<_>) = events.iter().partition(|e| e.starts_with("log"));
    assert_eq!(
        rest,
        [
            "item 1 Started",
            "state 1/0",
            "state 1/1",
            "item 1 Failed",
            "state idle"
        ]
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("synthesis failed for item 1 chunk 0: no usable Windows voices")),
        "{logs:?}"
    );
    assert_eq!(r.calls_now(), []);
    // Still answering.
    assert_eq!(r.h.get(Key::Engine).unwrap(), Value::Text("broken".into()));
}

#[test]
fn a_late_subscriber_still_hears_why_the_engine_is_not_ready() {
    let r = Rig::with(Arc::new(BrokenEngine));
    let why = "log engine 'broken' is not ready: no usable Windows voices";
    // The warm-up may end before or after the rig subscribed: told once.
    let first = fmt(&r.events.recv_timeout(TIMEOUT).expect("no warm-up log"));
    assert!(first.starts_with(why), "{first}");
    let late = r.h.subscribe().unwrap();
    r.h.state().unwrap();
    let got: Vec<_> = late.try_iter().map(|e| fmt(&e)).collect();
    assert_eq!(got, [first]);
    assert_eq!(r.events_now(), Vec::<String>::new());
}

#[test]
fn shutdown_cancels_an_engine_warm_up() {
    let engine = Arc::new(SlowWarmEngine::default());
    let r = Rig::with(engine.clone());
    engine.wait_warming();
    let h = r.h.clone();
    let (done, finished) = std::sync::mpsc::channel();
    thread::spawn(move || {
        h.shutdown();
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(TIMEOUT).is_ok(),
        "shutdown waited for the warm-up"
    );
}

#[test]
fn shutdown_while_playing_ends_items_and_closes_everything() {
    let (r, _) = Rig::new();
    speak(&r.h, THREE);
    speak(&r.h, "Queued.");
    r.calls(1);
    r.out.start();
    let clone = r.h.clone();
    r.h.shutdown();
    assert_eq!(
        r.events.iter().map(|e| fmt(&e)).collect::<Vec<_>>(),
        [
            "item 1 Started",
            "state 1/0",
            "state 1/0 queued 1",
            "item 1 Skipped",
            "item 2 Skipped",
            "state idle"
        ]
    );
    assert_eq!(r.out.take_calls(), [OutputCall::Stop]);
    assert_eq!(clone.control(Control::Play), Err(Error::Closed));
    assert_eq!(
        clone.speak("Too late.", QueueMode::Append, false, None),
        Err(Error::Closed)
    );
    assert_eq!(clone.get(Key::Volume), Err(Error::Closed));
    assert_eq!(clone.voices(None), Err(Error::Closed));
    assert!(clone.subscribe().is_err());
    clone.shutdown();
    r.h.shutdown();
}

#[test]
fn shutdown_while_synthesizing_cancels_and_returns() {
    let engine = Arc::new(GateEngine::default());
    let r = Rig::with(engine.clone());
    speak(&r.h, "Stuck in the engine.");
    engine.wait_started(1);
    r.h.shutdown();
    assert!(engine.cancels() >= 1);
    assert_eq!(r.h.state(), Err(Error::Closed));
}

#[test]
fn dropping_the_last_handle_shuts_the_reader_down() {
    let (r, _) = Rig::new();
    let events = r.events;
    speak(&r.h, "Bye.");
    let other = r.h.clone();
    drop(r.h);
    // A clone keeps it alive.
    assert!(other.state().is_ok());
    drop(other);
    let end = Instant::now() + TIMEOUT;
    loop {
        match events.recv_timeout(TIMEOUT) {
            Ok(_) => assert!(Instant::now() < end),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!("the reader did not shut down"),
        }
    }
}

/// A tiny deterministic generator, so a failing storm can be replayed.
fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

#[test]
fn rapid_control_storms_from_several_threads_leave_a_consistent_reader() {
    let (r, _) = Rig::new();
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
    ];
    // Audio keeps finishing whatever is loaded while the storm runs.
    let out = r.out.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let player = {
        let stop = stop.clone();
        thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                if let Some(gen) = out.loaded() {
                    out.send(AudioEvent::ChunkStarted { gen });
                    out.send(AudioEvent::ChunkFinished { gen });
                }
                thread::sleep(Duration::from_micros(200));
            }
        })
    };
    let storms: Vec<_> = (0..4u64)
        .map(|t| {
            let h = r.h.clone();
            thread::spawn(move || {
                let mut seed = t + 1;
                for i in 0..300 {
                    let n = lcg(&mut seed) as usize;
                    if n.is_multiple_of(10) {
                        let interrupt = n.is_multiple_of(20);
                        h.speak(
                            &format!("Storm {t} {i}. Second part. Third part."),
                            if n.is_multiple_of(3) {
                                QueueMode::Replace
                            } else {
                                QueueMode::Append
                            },
                            interrupt,
                            None,
                        )
                        .unwrap();
                    } else {
                        h.control(controls[n % controls.len()]).unwrap();
                    }
                }
            })
        })
        .collect();
    for s in storms {
        s.join().unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    player.join().unwrap();

    r.h.control(Control::Stop).unwrap();
    r.h.control(Control::Unmute).unwrap();
    let st = r.h.state().unwrap();
    assert!(st.now_playing.is_none() && st.queued == 0 && !st.paused && !st.muted);
    assert_eq!(r.out.loaded(), None);
    assert_eq!(r.out.volume(), 100);

    // Every item that started ended exactly once; states only moved on.
    let mut last_seq = 0;
    let mut ended: BTreeMap<ItemId, usize> = BTreeMap::new();
    let mut started = BTreeSet::new();
    for e in r.events.try_iter() {
        match e {
            Event::State(s) => {
                assert!(s.seq > last_seq, "seq {} after {last_seq}", s.seq);
                last_seq = s.seq;
            }
            Event::Item { item_id, phase } => {
                if phase == ItemPhase::Started {
                    assert!(started.insert(item_id), "{item_id:?} started twice");
                } else {
                    *ended.entry(item_id).or_default() += 1;
                }
            }
            Event::Log { message } => panic!("unexpected log: {message}"),
            Event::EngineStatus { .. } => panic!("the fake engine is always ready"),
        }
    }
    assert!(ended.values().all(|n| *n == 1), "{ended:?}");
    assert!(started.iter().all(|id| ended.contains_key(id)));

    // And it still reads.
    r.out.take_calls();
    let id = speak(&r.h, "After the storm.");
    let calls = r.calls(1);
    let gen = match calls.as_slice() {
        [OutputCall::Play { gen, .. }] => *gen,
        other => panic!("expected one play, got {other:?}"),
    };
    assert_eq!(calls, [play(id.0, 0, gen, len("After the storm.", 200))]);
}
