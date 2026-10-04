//! A simulated host for the reader state machine. It carries out every
//! `Effect` against a model of the audio output and checks the contract on
//! each call, so a test only has to drive the reader and look at the result.
#![allow(dead_code)]
use sonara_core::reader::{
    AudioEvent, Control, Effect, Event, ItemId, ItemPhase, QueueMode, Reader, State,
};
use std::collections::{HashMap, HashSet};

pub const THREE: &str = "One is here. Two is here. Three is here.";

/// Model of the audio output the effects drive.
#[derive(Debug, Default)]
struct Output {
    /// gen and (item, chunk) of the chunk loaded in the output.
    playing: Option<(u64, ItemId, usize)>,
    paused: bool,
    muted: bool,
    volume: u8,
}

pub struct Host {
    pub reader: Reader,
    out: Output,
    /// Every effect of the last call.
    pub last: Vec<Effect>,
    /// Every Item event so far, in order.
    pub items: Vec<(ItemId, ItemPhase)>,
    /// Every emitted state so far.
    pub states: Vec<State>,
    /// Every chunk whose audio finished, in order: what the listener heard.
    pub heard: Vec<(ItemId, usize)>,
    /// Synthesized chunks of items that are still alive.
    synthesized: HashSet<(ItemId, usize)>,
    /// Item ids that have started and not yet ended.
    live: HashSet<ItemId>,
    /// Item ids that ended (Finished, Skipped, Failed).
    ended: HashSet<ItemId>,
    texts: HashMap<(ItemId, usize), String>,
    gens: Vec<u64>,
}

impl Host {
    pub fn new() -> Self {
        let reader = Reader::new();
        let volume = reader.state().volume;
        Host {
            reader,
            out: Output {
                volume,
                ..Output::default()
            },
            last: Vec::new(),
            items: Vec::new(),
            states: Vec::new(),
            heard: Vec::new(),
            synthesized: HashSet::new(),
            live: HashSet::new(),
            ended: HashSet::new(),
            texts: HashMap::new(),
            gens: Vec::new(),
        }
    }

    pub fn speak(&mut self, text: &str) -> ItemId {
        self.speak_with(text, QueueMode::Append, false)
    }

    pub fn speak_with(&mut self, text: &str, mode: QueueMode, interrupt: bool) -> ItemId {
        let (id, fx) = self.reader.speak(text, mode, interrupt, None);
        self.apply(fx);
        id
    }

    /// Chunks to synthesize ahead of the playing one.
    pub fn lookahead(&mut self, n: usize) {
        let fx = self.reader.set_lookahead(n);
        self.apply(fx);
    }

    pub fn ctl(&mut self, c: Control) {
        let fx = self.reader.control(c);
        self.apply(fx);
    }

    pub fn audio(&mut self, e: AudioEvent) {
        let fx = self.reader.on_audio(e);
        self.apply(fx);
    }

    pub fn gen(&self) -> Option<u64> {
        self.out.playing.map(|p| p.0)
    }

    /// (item, chunk) loaded in the output.
    pub fn playing(&self) -> Option<(ItemId, usize)> {
        self.out.playing.map(|p| (p.1, p.2))
    }

    pub fn output_paused(&self) -> bool {
        self.out.paused
    }

    pub fn output_muted(&self) -> bool {
        self.out.muted
    }

    pub fn output_volume(&self) -> u8 {
        self.out.volume
    }

    /// The loaded chunk starts and plays to its end.
    pub fn finish_chunk(&mut self) {
        let (gen, item, chunk) = self.out.playing.expect("finish_chunk: nothing playing");
        assert!(!self.out.paused, "finish_chunk while the output is paused");
        self.audio(AudioEvent::ChunkStarted { gen });
        self.out.playing = None;
        self.heard.push((item, chunk));
        self.audio(AudioEvent::ChunkFinished { gen });
    }

    /// The loaded chunk ends in a race with a pause: the output finished it
    /// before it saw PauseOutput.
    pub fn finish_chunk_racing_pause(&mut self) {
        let (gen, item, chunk) = self.out.playing.expect("nothing playing");
        self.out.playing = None;
        self.out.paused = false;
        self.heard.push((item, chunk));
        self.audio(AudioEvent::ChunkFinished { gen });
    }

    /// The loaded chunk fails before it starts.
    pub fn fail_chunk(&mut self) {
        let (gen, _, _) = self.out.playing.expect("fail_chunk: nothing playing");
        self.out.playing = None;
        self.out.paused = false;
        self.audio(AudioEvent::Failed {
            gen,
            reason: "engine error".into(),
        });
    }

    /// Play everything until the reader is idle (or paused); bounded.
    pub fn play_out(&mut self) {
        for _ in 0..10_000 {
            if self.out.playing.is_none() || self.out.paused {
                return;
            }
            self.finish_chunk();
        }
        panic!("play_out did not end");
    }

    pub fn state(&self) -> State {
        self.reader.state()
    }

    pub fn emitted_states(&self) -> usize {
        self.last
            .iter()
            .filter(|e| matches!(e, Effect::Emit(Event::State(_))))
            .count()
    }

    pub fn plays_in_last(&self) -> Vec<(ItemId, usize)> {
        self.last
            .iter()
            .filter_map(|e| match e {
                Effect::PlayChunk { item, chunk, .. } => Some((*item, *chunk)),
                _ => None,
            })
            .collect()
    }

    pub fn item_events_in_last(&self) -> Vec<(ItemId, ItemPhase)> {
        self.last
            .iter()
            .filter_map(|e| match e {
                Effect::Emit(Event::Item { item_id, phase }) => Some((*item_id, *phase)),
                _ => None,
            })
            .collect()
    }

    pub fn synth_in_last(&self) -> Vec<(ItemId, usize, String)> {
        self.last
            .iter()
            .filter_map(|e| match e {
                Effect::Synthesize { item, chunk, text } => Some((*item, *chunk, text.clone())),
                _ => None,
            })
            .collect()
    }

    /// Carry out `fx` against the output model, checking the contract.
    fn apply(&mut self, fx: Vec<Effect>) {
        let mut states_here = 0;
        for e in &fx {
            match e {
                Effect::Synthesize { item, chunk, text } => {
                    assert!(
                        self.synthesized.insert((*item, *chunk)),
                        "Synthesize twice for {:?}/{} while alive",
                        item,
                        chunk
                    );
                    assert!(!text.trim().is_empty(), "Synthesize of empty text");
                    assert!(
                        !self.ended.contains(item),
                        "Synthesize for {item:?}, which already ended"
                    );
                    self.texts.insert((*item, *chunk), text.clone());
                }
                Effect::PlayChunk { item, chunk, gen } => {
                    assert!(
                        self.out.playing.is_none(),
                        "PlayChunk {:?}/{} while {:?} is loaded",
                        item,
                        chunk,
                        self.out.playing
                    );
                    assert!(
                        self.synthesized.contains(&(*item, *chunk)),
                        "PlayChunk {:?}/{} without Synthesize",
                        item,
                        chunk
                    );
                    if let Some(prev) = self.max_gen() {
                        assert!(*gen > prev, "gen must increase");
                    }
                    self.gens.push(*gen);
                    self.out.playing = Some((*gen, *item, *chunk));
                    self.out.paused = false;
                }
                Effect::PauseOutput => {
                    assert!(
                        self.out.playing.is_some(),
                        "PauseOutput with nothing loaded"
                    );
                    assert!(!self.out.paused, "PauseOutput twice");
                    self.out.paused = true;
                }
                Effect::ResumeOutput => {
                    assert!(
                        self.out.playing.is_some(),
                        "ResumeOutput with nothing loaded"
                    );
                    assert!(self.out.paused, "ResumeOutput while not paused");
                    self.out.paused = false;
                }
                Effect::StopOutput => {
                    assert!(self.out.playing.is_some(), "StopOutput with nothing loaded");
                    self.out.playing = None;
                    self.out.paused = false;
                }
                Effect::Mute => {
                    assert!(!self.out.muted, "Mute twice");
                    self.out.muted = true;
                }
                Effect::Unmute => {
                    assert!(self.out.muted, "Unmute while not muted");
                    self.out.muted = false;
                }
                Effect::SetVolume(v) => {
                    assert_ne!(*v, self.out.volume, "SetVolume without a change");
                    self.out.volume = *v;
                }
                Effect::Emit(Event::State(s)) => {
                    states_here += 1;
                    if let Some(prev) = self.states.last() {
                        assert!(s.seq > prev.seq, "state seq must increase");
                        let mut p = prev.clone();
                        p.seq = s.seq;
                        assert_ne!(&p, s, "state emitted without a change");
                    }
                    self.states.push(s.clone());
                }
                Effect::Emit(Event::Item { item_id, phase }) => {
                    self.items.push((*item_id, *phase));
                    match phase {
                        ItemPhase::Started => {
                            assert!(self.live.insert(*item_id), "Started twice");
                        }
                        _ => {
                            self.live.remove(item_id);
                            self.ended.insert(*item_id);
                            self.synthesized.retain(|(i, _)| i != item_id);
                        }
                    }
                }
            }
        }
        assert!(states_here <= 1, "more than one state event per call");
        self.last = fx;
        self.check_invariants();
    }

    fn max_gen(&self) -> Option<u64> {
        self.gens.last().copied()
    }

    fn check_invariants(&self) {
        let s = self.reader.state();
        if let Some(last) = self.states.last() {
            assert_eq!(&s, last, "state() differs from the last emitted state");
        } else {
            assert_eq!(s.seq, 0);
        }
        match &s.now_playing {
            None => {
                assert!(!s.paused, "paused while idle");
                assert_eq!(s.queued, 0, "items queued while idle");
                assert!(self.out.playing.is_none(), "output loaded while idle");
            }
            Some(np) => {
                assert!(np.chunk < np.chunks);
                assert!(self.live.contains(&np.item_id), "now playing not Started");
                if s.paused {
                    if let Some((_, item, chunk)) = self.out.playing {
                        assert!(self.out.paused, "state paused, output playing");
                        assert_eq!((item, chunk), (np.item_id, np.chunk));
                    }
                } else {
                    assert!(
                        !self.out.paused,
                        "output paused, state playing (stuck pause)"
                    );
                    let (_, item, chunk) = self
                        .out
                        .playing
                        .expect("playing state with nothing loaded (stuck)");
                    assert_eq!((item, chunk), (np.item_id, np.chunk));
                }
            }
        }
        assert_eq!(s.muted, self.out.muted);
    }
}
