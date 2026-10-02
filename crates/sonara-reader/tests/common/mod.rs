//! A reader on the fake engine (or a test engine) and `TestOutput`. The test
//! decides when audio starts and ends; the helpers wait for the worker and
//! synthesis threads with a timeout, so every assertion is deterministic.
#![allow(dead_code)]
pub mod engines;

use sonara_audio::{OutputCall, TestOutput};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Engine, Event, ItemId, ReaderHandle, Registry};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const THREE: &str = "One is here. Two is here. Three is here.";
pub const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Rig {
    pub h: ReaderHandle,
    pub out: TestOutput,
    pub events: Receiver<Event>,
}

impl Rig {
    pub fn new() -> (Self, Arc<FakeEngine>) {
        let engine = Arc::new(FakeEngine::new());
        (Self::with(engine.clone()), engine)
    }

    pub fn with(engine: Arc<dyn Engine>) -> Self {
        let mut registry = Registry::default();
        registry.register(engine).unwrap();
        Self::config(Config::new(registry))
    }

    pub fn config(config: Config) -> Self {
        let (out, rx) = TestOutput::new();
        let h = ReaderHandle::new(config.with_output(Box::new(out.clone()), rx)).unwrap();
        let events = h.subscribe().unwrap();
        // The startup state events came before the subscription.
        Rig { h, out, events }
    }

    /// Wait until the output received `n` calls since the last take, and
    /// return them (more if more came).
    pub fn calls(&self, n: usize) -> Vec<OutputCall> {
        let mut got = Vec::new();
        let end = Instant::now() + TIMEOUT;
        loop {
            got.extend(self.out.take_calls());
            if got.len() >= n {
                return got;
            }
            assert!(Instant::now() < end, "waited for {n} calls, got {got:?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The calls so far, after a round trip through the worker.
    pub fn calls_now(&self) -> Vec<OutputCall> {
        self.h.state().unwrap();
        self.out.take_calls()
    }

    /// The events so far (after a round trip), in compact notation.
    pub fn events_now(&self) -> Vec<String> {
        self.h.state().unwrap();
        self.events.try_iter().map(|e| fmt(&e)).collect()
    }

    /// Events until one equal to `last` (in compact notation), inclusive.
    pub fn events_until(&self, last: &str) -> Vec<String> {
        let mut got = Vec::new();
        loop {
            let e = self
                .events
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("waited for '{last}', got {got:?}"));
            got.push(fmt(&e));
            if got.last().map(String::as_str) == Some(last) {
                return got;
            }
        }
    }

    /// Wait until the output has a chunk loaded.
    pub fn loaded(&self) -> u64 {
        let end = Instant::now() + TIMEOUT;
        loop {
            if let Some(gen) = self.out.loaded() {
                return gen;
            }
            assert!(Instant::now() < end, "nothing was loaded");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

pub fn fmt(e: &Event) -> String {
    match e {
        Event::Item { item_id, phase } => format!("item {} {:?}", item_id.0, phase),
        Event::State(st) => match &st.now_playing {
            Some(np) => format!(
                "state {}/{}{}{}{}",
                np.item_id.0,
                np.chunk,
                if st.paused { " paused" } else { "" },
                if st.muted { " muted" } else { "" },
                if st.queued > 0 {
                    format!(" queued {}", st.queued)
                } else {
                    String::new()
                }
            ),
            None => format!("state idle{}", if st.muted { " muted" } else { "" }),
        },
        Event::Log { message } => format!("log {message}"),
    }
}

/// Samples the fake engine makes for `text` at `rate`.
pub fn len(text: &str, rate: u32) -> usize {
    FakeEngine::render(text, "", rate).unwrap().len()
}

pub fn play(item: u64, chunk: usize, gen: u64, samples: usize) -> OutputCall {
    OutputCall::Play {
        item: ItemId(item),
        chunk,
        gen,
        samples,
    }
}
