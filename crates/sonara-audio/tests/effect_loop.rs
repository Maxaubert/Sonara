//! The reader, the fake engine and `TestOutput` wired through the example's
//! effect loop. The test decides when audio starts and ends, so every
//! effect, audio event and output call is asserted in exact order.
#[path = "../examples/driver/mod.rs"]
mod driver;

use driver::{Driver, Step};
use sonara_audio::{AudioEvent, ItemId, OutputCall, TestOutput};
use sonara_core::reader::{Control, Effect, Event};
use sonara_engine::fake::FakeEngine;
use sonara_engine::Engine;
use std::sync::Arc;

const THREE: &str = "One is here. Two is here. Three is here.";

struct Rig {
    d: Driver<TestOutput>,
    out: TestOutput,
    engine: Arc<FakeEngine>,
    seen: usize,
}

impl Rig {
    fn new() -> Self {
        let (out, events) = TestOutput::new();
        let engine = Arc::new(FakeEngine::new());
        let d = Driver::new(engine.clone() as Arc<dyn Engine>, out.clone(), events);
        Rig {
            d,
            out,
            engine,
            seen: 0,
        }
    }

    /// The loop steps since the last call, in a compact notation.
    fn steps(&mut self) -> Vec<String> {
        let new = self.d.log[self.seen..].iter().map(fmt).collect();
        self.seen = self.d.log.len();
        new
    }

    /// Output calls since the last call.
    fn calls(&self) -> Vec<OutputCall> {
        self.out.take_calls()
    }
}

fn fmt(s: &Step) -> String {
    match s {
        Step::Fx(Effect::Synthesize { item, chunk, .. }) => format!("synth {}/{}", item.0, chunk),
        Step::Fx(Effect::PlayChunk { item, chunk, gen }) => {
            format!("play {}/{} g{}", item.0, chunk, gen)
        }
        Step::Fx(Effect::PauseOutput) => "pause".into(),
        Step::Fx(Effect::ResumeOutput) => "resume".into(),
        Step::Fx(Effect::StopOutput) => "stop".into(),
        Step::Fx(Effect::Mute) => "mute".into(),
        Step::Fx(Effect::Unmute) => "unmute".into(),
        Step::Fx(Effect::SetVolume(v)) => format!("volume {v}"),
        Step::Fx(Effect::Emit(Event::Item { item_id, phase })) => {
            format!("item {} {:?}", item_id.0, phase)
        }
        Step::Fx(Effect::Emit(Event::State(st))) => match &st.now_playing {
            Some(np) => format!(
                "state {}/{}{}{}",
                np.item_id.0,
                np.chunk,
                if st.paused { " paused" } else { "" },
                if st.queued > 0 {
                    format!(" queued {}", st.queued)
                } else {
                    String::new()
                }
            ),
            None => "state idle".into(),
        },
        Step::Audio(AudioEvent::ChunkStarted { gen }) => format!("started g{gen}"),
        Step::Audio(AudioEvent::ChunkFinished { gen }) => format!("finished g{gen}"),
        Step::Audio(AudioEvent::Failed { gen, .. }) => format!("failed g{gen}"),
    }
}

/// Samples the fake engine makes for `text` at `rate`.
fn len(text: &str, rate: u32) -> usize {
    FakeEngine::render(text, "", rate).unwrap().len()
}

fn play(item: u64, chunk: usize, gen: u64, samples: usize) -> OutputCall {
    OutputCall::Play {
        item: ItemId(item),
        chunk,
        gen,
        samples,
    }
}

#[test]
fn speak_pause_resume_next_restart_skip_stop() {
    let mut r = Rig::new();
    let (one, two, three) = ("One is here.", "Two is here.", "Three is here.");

    // speak: the first chunk plays, the second is synthesized ahead.
    let id = r.d.speak(THREE);
    assert_eq!(id, ItemId(1));
    assert_eq!(
        r.steps(),
        [
            "item 1 Started",
            "synth 1/0",
            "play 1/0 g1",
            "synth 1/1",
            "state 1/0"
        ]
    );
    assert_eq!(r.calls(), [play(1, 0, 1, len(one, 200))]);

    // The audio starts: nothing visible changes.
    r.out.start();
    r.d.pump();
    assert_eq!(r.steps(), ["started g1"]);

    // Pause mid-chunk, then resume the same chunk where it was.
    r.d.control(Control::Pause);
    assert_eq!(r.steps(), ["pause", "state 1/0 paused"]);
    assert_eq!(r.calls(), [OutputCall::Pause]);
    assert!(r.out.paused());
    r.d.control(Control::Play);
    assert_eq!(r.steps(), ["resume", "state 1/0"]);
    assert_eq!(r.calls(), [OutputCall::Resume]);

    // The chunk ends: the next one plays, the last one is synthesized.
    r.out.finish();
    r.d.pump();
    assert_eq!(
        r.steps(),
        ["finished g1", "play 1/1 g2", "synth 1/2", "state 1/1"]
    );
    assert_eq!(r.calls(), [play(1, 1, 2, len(two, 200))]);

    // Next: cut chunk 1, play chunk 2 (already synthesized).
    r.d.control(Control::Next);
    assert_eq!(r.steps(), ["stop", "play 1/2 g3", "state 1/2"]);
    assert_eq!(
        r.calls(),
        [OutputCall::Stop, play(1, 2, 3, len(three, 200))]
    );

    // Restart: back to chunk 0 from the cache, no new synthesis.
    r.d.control(Control::Restart);
    assert_eq!(r.steps(), ["stop", "play 1/0 g4", "state 1/0"]);
    assert_eq!(r.calls(), [OutputCall::Stop, play(1, 0, 4, len(one, 200))]);
    assert_eq!(r.engine.syntheses(), 3);

    // A second item queues behind the first.
    let second = r.d.speak("Second item.");
    assert_eq!(second, ItemId(2));
    assert_eq!(r.steps(), ["state 1/0 queued 1"]);
    assert_eq!(r.calls(), []);

    // Skip: the first item ends, the second starts.
    r.d.control(Control::Skip);
    assert_eq!(
        r.steps(),
        [
            "stop",
            "item 1 Skipped",
            "item 2 Started",
            "synth 2/0",
            "play 2/0 g5",
            "state 2/0"
        ]
    );
    assert_eq!(
        r.calls(),
        [OutputCall::Stop, play(2, 0, 5, len("Second item.", 200))]
    );
    assert_eq!(r.d.cached(), 1);

    // A late event from a superseded play changes nothing.
    r.out.send(AudioEvent::ChunkFinished { gen: 4 });
    r.d.pump();
    assert_eq!(r.steps(), ["finished g4"]);
    assert_eq!(r.calls(), []);

    // Stop: everything ends, the kept audio is dropped.
    r.d.control(Control::Stop);
    assert_eq!(r.steps(), ["stop", "item 2 Skipped", "state idle"]);
    assert_eq!(r.calls(), [OutputCall::Stop]);
    assert!(r.d.idle());
    assert_eq!(r.d.cached(), 0);
    assert_eq!(r.out.loaded(), None);
    assert!(r.d.failures.is_empty());
}

#[test]
fn pause_between_chunks_holds_the_next_chunk_until_play() {
    let mut r = Rig::new();
    r.d.speak(THREE);
    r.steps();
    r.calls();
    r.d.control(Control::Pause);
    // The chunk was already ending when the pause arrived.
    r.out.finish();
    r.d.pump();
    assert_eq!(
        r.steps(),
        [
            "pause",
            "state 1/0 paused",
            "finished g1",
            "state 1/1 paused"
        ]
    );
    assert_eq!(r.calls(), [OutputCall::Pause]);
    r.d.control(Control::Play);
    assert_eq!(r.steps(), ["play 1/1 g2", "synth 1/2", "state 1/1"]);
    assert_eq!(r.calls(), [play(1, 1, 2, len("Two is here.", 200))]);
}

#[test]
fn a_failed_synthesis_skips_its_chunk_and_the_item_continues() {
    let mut r = Rig::new();
    let text = "Good one here. Bad [fail] one. Last one here.";
    r.d.speak(text);
    assert_eq!(
        r.steps(),
        [
            "item 1 Started",
            "synth 1/0",
            "play 1/0 g1",
            "synth 1/1",
            "state 1/0"
        ]
    );
    assert_eq!(r.d.failures.len(), 1);
    assert!(
        r.d.failures[0].1.starts_with("fake engine failure"),
        "{:?}",
        r.d.failures
    );
    r.calls();

    r.out.finish();
    r.d.pump();
    assert_eq!(
        r.steps(),
        [
            "finished g1",
            "play 1/1 g2",
            "synth 1/2",
            "state 1/1",
            "failed g2",
            "play 1/2 g3",
            "state 1/2"
        ]
    );
    // The failed chunk never reached the output.
    assert_eq!(r.calls(), [play(1, 2, 3, len("Last one here.", 200))]);

    r.out.finish();
    r.d.pump();
    assert_eq!(r.steps(), ["finished g3", "item 1 Finished", "state idle"]);
}

#[test]
fn a_device_failure_fails_the_item_without_a_panic() {
    let mut r = Rig::new();
    r.out.fail_plays(Some("device gone"));
    r.d.speak("Only chunk.");
    r.d.pump();
    assert_eq!(
        r.steps(),
        [
            "item 1 Started",
            "synth 1/0",
            "play 1/0 g1",
            "state 1/0",
            "failed g1",
            "item 1 Failed",
            "state idle"
        ]
    );
    assert!(r.d.idle());
}

#[test]
fn mute_and_volume_reach_the_output() {
    let mut r = Rig::new();
    r.d.control(Control::Mute);
    assert_eq!(r.calls(), [OutputCall::SetVolume(0)]);
    // A new level while muted is kept for unmute, the output stays silent.
    r.d.with_reader(|reader| reader.set_volume(60));
    assert_eq!(r.calls(), []);
    assert_eq!(r.out.volume(), 0);
    r.d.control(Control::Unmute);
    assert_eq!(r.calls(), [OutputCall::SetVolume(60)]);
    r.d.with_reader(|reader| reader.set_volume(80));
    assert_eq!(r.calls(), [OutputCall::SetVolume(80)]);
    assert_eq!(
        r.steps(),
        [
            "mute",
            "state idle",
            "volume 60",
            "state idle",
            "unmute",
            "state idle",
            "volume 80",
            "state idle"
        ]
    );
}

#[test]
fn rate_reaches_the_engine() {
    let mut r = Rig::new();
    r.d.with_reader(|reader| reader.set_rate(400));
    r.d.speak("Fast words.");
    assert_eq!(r.calls(), [play(1, 0, 1, len("Fast words.", 400))]);
    assert_eq!(len("Fast words.", 400) * 2, len("Fast words.", 200));
}
