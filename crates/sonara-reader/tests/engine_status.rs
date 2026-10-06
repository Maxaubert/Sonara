//! The engine's readiness reaches subscribers (`Event::EngineStatus`) and
//! `engine_status`: a Kokoro-style engine downloading its model, then
//! ready. Engines that are always ready send nothing.
mod common;

use common::engines::StatusEngine;
use common::Rig;
use sonara_engine::{EngineId, EngineStatus, Readiness};
use sonara_reader::Event;
use std::sync::Arc;
use std::time::Duration;

fn downloading(done: u64) -> EngineStatus {
    EngineStatus {
        readiness: Readiness::Downloading,
        progress: Some((done, 100)),
        fallback: Some(EngineId("onecore")),
        message: None,
        reason: None,
    }
}

fn next_status(rig: &Rig) -> EngineStatus {
    match rig.events.recv_timeout(Duration::from_secs(5)) {
        Ok(Event::EngineStatus { engine, status, .. }) => {
            assert_eq!(engine, EngineId("status"));
            status
        }
        other => panic!("expected an engine status event, got {other:?}"),
    }
}

#[test]
fn engine_status_changes_are_sent_to_subscribers() {
    let engine = Arc::new(StatusEngine::new(downloading(10)));
    let rig = Rig::with(engine.clone());
    assert_eq!(rig.h.engine_status().unwrap(), downloading(10));
    // Progress: a status event, no log line.
    engine.set(downloading(60));
    assert_eq!(next_status(&rig), downloading(60));
    // Readiness: a log line, then the status event.
    engine.set(EngineStatus::ready());
    assert_eq!(
        rig.events_until("engine status ready"),
        ["log engine 'status' is ready", "engine status ready"]
    );
    assert_eq!(rig.h.engine_status().unwrap().readiness, Readiness::Ready);
}

#[test]
fn status_changes_are_counted_once_for_every_subscriber() {
    let engine = Arc::new(StatusEngine::new(downloading(10)));
    let rig = Rig::with(engine.clone());
    assert_eq!(rig.h.engine_status_changes().unwrap(), (downloading(10), 0));
    engine.set(downloading(20));
    next_status(&rig);
    engine.set(downloading(30));
    next_status(&rig);
    // A subscriber that comes later sees the same count: it is the
    // reader's, not the subscription's.
    let late = rig.h.subscribe().unwrap();
    assert_eq!(rig.h.engine_status_changes().unwrap(), (downloading(30), 2));
    engine.set(downloading(40));
    let changes = |rx: &std::sync::mpsc::Receiver<Event>| loop {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::EngineStatus { changes, .. } => return changes,
            _ => continue,
        }
    };
    assert_eq!(changes(&rig.events), 3);
    assert_eq!(changes(&late), 3);
}

#[test]
fn an_always_ready_engine_sends_no_status_events() {
    let (rig, _) = Rig::new();
    std::thread::sleep(Duration::from_millis(600));
    assert!(rig.events_now().iter().all(|e| !e.starts_with("engine ")));
    assert_eq!(rig.h.engine_status().unwrap(), EngineStatus::ready());
}

/// #274: OneCore with no usable voices said `ready` in `engine_status`
/// while every item failed. A failed warm-up of an engine that calls itself
/// ready is reported `unavailable`, with why, until it speaks.
#[test]
fn an_engine_whose_warm_up_failed_is_not_reported_ready() {
    let engine = Arc::new(common::engines::ColdEngine::default());
    let rig = Rig::with(engine.clone());
    let events = rig.events_until("engine cold unavailable");
    assert!(
        events
            .iter()
            .any(|e| e.starts_with("log engine 'cold' is not ready: no usable Windows voices")),
        "{events:?}"
    );
    let status = rig.h.engine_status().unwrap();
    assert_eq!(status.readiness, Readiness::Unavailable);
    assert!(
        status
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with("no usable Windows voices")),
        "{status:?}"
    );
    // Voices installed meanwhile: the first sentence it speaks makes it ready.
    engine
        .speaks
        .store(true, std::sync::atomic::Ordering::SeqCst);
    rig.h
        .speak("Hello.", sonara_reader::QueueMode::Append, false, None)
        .unwrap();
    rig.events_until("engine cold ready");
    assert_eq!(rig.h.engine_status().unwrap(), EngineStatus::ready());
}
