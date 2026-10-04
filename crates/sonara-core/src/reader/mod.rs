//! L1 reader state machine: one queue of items, playback controls, state and
//! item events.
//!
//! PURE: no threads, no I/O, no clock. Every input (`speak`, `control`,
//! `on_audio`, the setters) returns the `Effect`s the host must carry out, in
//! order; the reader never waits for anything. Time-dependent behaviour comes
//! in as explicit `AudioEvent`s.
//!
//! Semantics (binding for the facade and protocol v1 `control`):
//! - `speak` splits the text into chunks (`split_chunks`). Text with no
//!   speakable chunk is a no-op: it gets a fresh id, reported `Skipped`, and
//!   changes nothing else (mode and interrupt are ignored).
//! - `Append` queues after everything; `Replace` first drops the unread items
//!   (each `Skipped`), never the current one; `interrupt` also cuts the
//!   current item (`StopOutput`, `Skipped`) and starts the new one at once,
//!   ahead of whatever is still queued.
//! - `Pause`, `Play` and `Toggle` act only on a current item; when idle they
//!   are no-ops, so the reader is never paused with nothing to read. A pause
//!   holds across navigation, `Skip` and `interrupt`: the new chunk or item
//!   waits for `Play`. `Skip` with nothing left goes idle and unpaused.
//! - `Previous`/`Next` move one chunk within the current item; `Previous` on
//!   the first chunk restarts it and `Next` on the last chunk is `Skip`.
//!   `Restart` goes to chunk 0; when idle it replays the last item that ended
//!   (not after `Stop`) as a new item with a fresh id. `Skip` ends the current item (`Skipped`) and starts
//!   the next. `Stop` clears the current item and the queue (all `Skipped`).
//! - `Mute`/`Unmute` emit `Effect::Mute`/`Effect::Unmute`; the host sets the
//!   output volume to zero and back. Playback keeps moving while muted, and
//!   the mute survives `Stop` and new items.
//! - Prefetch: whenever a chunk plays, the `lookahead` chunks after it in
//!   play order (the rest of the current item, then the queued items' chunks)
//!   are synthesized ahead; 1 by default, up to 4 (`set_lookahead`, for an
//!   engine with a slow round trip).
//! - Joined chunks (`set_chunk_chars`, #235): for an engine that asks for
//!   longer chunks (one billed per request), the sentences of an item
//!   spoken from then on are joined by `join_chunks`; the first chunk stays
//!   one sentence. 0 (the default) keeps one sentence per chunk. Without
//!   quick start (`set_quick_start(false)`) every chunk joins up to the
//!   limit (`join_whole`), so a reply under it is one chunk.
//! - Every `PlayChunk` carries a new `gen`; an `AudioEvent` whose `gen` is not
//!   the chunk loaded in the output is stale and ignored, so a superseded
//!   play can never move the reader. A chunk that finishes while a pause is
//!   on its way still advances the reader, which stays paused.
//! - A failed chunk (synthesis or playback) is skipped and the item
//!   continues. The item ends `Failed` only when it reaches its end without
//!   any chunk having played; otherwise it ends `Finished`.
//! - `Event::State` is emitted only on an actual change, at most once per
//!   call and as the call's last effect, with a strictly increasing `seq`.
mod chunks;
mod types;

pub use chunks::{join_chunks, join_whole, split_chunks, JOIN_MIN};
pub use types::*;

use std::collections::{HashSet, VecDeque};

/// The deepest prefetch an engine may ask for.
pub const MAX_LOOKAHEAD: usize = 4;

#[derive(Debug)]
struct Current {
    item: Item,
    chunk: usize,
    /// Some chunk of this item reported ChunkStarted or ChunkFinished.
    played_any: bool,
}

/// The reader state machine. See the module docs for the semantics.
#[derive(Debug)]
pub struct Reader {
    next_id: u64,
    /// The last `gen` handed out.
    gen: u64,
    current: Option<Current>,
    queue: VecDeque<Item>,
    /// The last item that ended other than by Stop, for Restart when idle.
    last: Option<Item>,
    paused: bool,
    /// `gen` of the chunk loaded in the output (playing or paused there).
    loaded: Option<u64>,
    muted: bool,
    volume: u8,
    rate: u32,
    voice: Option<String>,
    /// Chunks already sent to Synthesize, for items still alive.
    requested: HashSet<(ItemId, usize)>,
    /// Chunks synthesized ahead of the playing one (1..=4).
    lookahead: usize,
    /// Join sentences into chunks of up to this many characters (0: no).
    chunk_chars: usize,
    /// The first joined chunk is one sentence (`join_chunks`); false joins
    /// every chunk up to `chunk_chars` (`join_whole`).
    quick_start: bool,
    /// The last state emitted (or the initial one, seq 0).
    shown: State,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    /// An idle reader: volume 100, rate 200 and the engine's default voice,
    /// as the Python reader.
    pub fn new() -> Self {
        let mut r = Reader {
            next_id: 1,
            gen: 0,
            current: None,
            queue: VecDeque::new(),
            last: None,
            paused: false,
            loaded: None,
            muted: false,
            volume: 100,
            rate: 200,
            voice: None,
            requested: HashSet::new(),
            lookahead: 1,
            chunk_chars: 0,
            quick_start: true,
            shown: State {
                seq: 0,
                now_playing: None,
                queued: 0,
                paused: false,
                muted: false,
                volume: 0,
                rate: 0,
                voice: None,
            },
        };
        r.shown = r.snapshot(0);
        r
    }

    /// Add `text` as one item. Returns its id and the effects.
    pub fn speak(
        &mut self,
        text: &str,
        mode: QueueMode,
        interrupt: bool,
        label: Option<String>,
    ) -> (ItemId, Vec<Effect>) {
        let id = ItemId(self.next_id);
        self.next_id += 1;
        let mut fx = Vec::new();
        let chunks = if self.quick_start {
            join_chunks(split_chunks(text), self.chunk_chars)
        } else {
            join_whole(split_chunks(text), self.chunk_chars)
        };
        if chunks.is_empty() {
            emit_item(&mut fx, id, ItemPhase::Skipped);
            return (id, fx);
        }
        let item = Item {
            id,
            label,
            text: text.to_string(),
            chunks,
        };
        if mode == QueueMode::Replace {
            while let Some(dropped) = self.queue.pop_front() {
                self.forget(&mut fx, dropped.id, ItemPhase::Skipped);
            }
        }
        if interrupt && self.current.is_some() {
            self.end_current(&mut fx, ItemPhase::Skipped);
            self.queue.push_front(item);
            self.start_next(&mut fx);
        } else {
            self.queue.push_back(item);
            if self.current.is_none() {
                self.start_next(&mut fx);
            }
        }
        (id, self.finish(fx))
    }

    pub fn control(&mut self, c: Control) -> Vec<Effect> {
        let mut fx = Vec::new();
        match c {
            Control::Play => {
                if self.paused {
                    self.resume(&mut fx);
                }
            }
            Control::Pause => self.pause(&mut fx),
            Control::Toggle => {
                if self.paused {
                    self.resume(&mut fx);
                } else {
                    self.pause(&mut fx);
                }
            }
            Control::Stop => self.stop(&mut fx),
            Control::Skip => {
                if self.current.is_some() {
                    self.skip(&mut fx);
                }
            }
            Control::Previous => {
                if let Some(cur) = &self.current {
                    let to = cur.chunk.saturating_sub(1);
                    self.move_to(&mut fx, to);
                }
            }
            Control::Next => {
                if let Some(cur) = &self.current {
                    if cur.chunk + 1 < cur.item.chunks.len() {
                        let to = cur.chunk + 1;
                        self.move_to(&mut fx, to);
                    } else {
                        self.skip(&mut fx);
                    }
                }
            }
            Control::Restart => {
                if self.current.is_some() {
                    self.move_to(&mut fx, 0);
                } else if let Some(mut item) = self.last.take() {
                    // A replay is a new item: the old id already got its
                    // final Event::Item and ids never repeat.
                    item.id = ItemId(self.next_id);
                    self.next_id += 1;
                    self.queue.push_front(item);
                    self.start_next(&mut fx);
                }
            }
            Control::Mute => {
                if !self.muted {
                    self.muted = true;
                    fx.push(Effect::Mute);
                }
            }
            Control::Unmute => {
                if self.muted {
                    self.muted = false;
                    fx.push(Effect::Unmute);
                }
            }
        }
        self.finish(fx)
    }

    /// Feed back what the output reports about a `PlayChunk`.
    pub fn on_audio(&mut self, e: AudioEvent) -> Vec<Effect> {
        let gen = match &e {
            AudioEvent::ChunkStarted { gen }
            | AudioEvent::ChunkFinished { gen }
            | AudioEvent::Failed { gen, .. } => *gen,
        };
        if self.loaded != Some(gen) {
            return Vec::new();
        }
        let mut fx = Vec::new();
        match e {
            AudioEvent::ChunkStarted { .. } => {
                if let Some(cur) = &mut self.current {
                    cur.played_any = true;
                }
            }
            AudioEvent::ChunkFinished { .. } => {
                self.loaded = None;
                if let Some(cur) = &mut self.current {
                    cur.played_any = true;
                }
                self.advance(&mut fx);
            }
            AudioEvent::Failed { .. } => {
                self.loaded = None;
                self.advance(&mut fx);
            }
        }
        self.finish(fx)
    }

    /// Speech volume in percent; the host validates the range.
    pub fn set_volume(&mut self, volume: u8) -> Vec<Effect> {
        let mut fx = Vec::new();
        if volume != self.volume {
            self.volume = volume;
            fx.push(Effect::SetVolume(volume));
        }
        self.finish(fx)
    }

    /// Speech rate for the next `Synthesize`; chunks already synthesized keep
    /// theirs. The host validates the range.
    pub fn set_rate(&mut self, rate: u32) -> Vec<Effect> {
        self.rate = rate;
        self.finish(Vec::new())
    }

    /// Voice for the next `Synthesize`; `None` is the engine default.
    pub fn set_voice(&mut self, voice: Option<String>) -> Vec<Effect> {
        self.voice = voice;
        self.finish(Vec::new())
    }

    /// How many chunks to synthesize ahead of the playing one, clamped to
    /// `1..=MAX_LOOKAHEAD`. A larger depth requests the newly allowed chunks
    /// at once when something plays; a smaller one cancels nothing.
    pub fn set_lookahead(&mut self, n: usize) -> Vec<Effect> {
        self.lookahead = n.clamp(1, MAX_LOOKAHEAD);
        self.finish(Vec::new())
    }

    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    /// Join the sentences of items spoken from now on into chunks of up to
    /// `n` characters (`join_chunks`; 0: one sentence per chunk). Items
    /// already queued keep their chunks.
    pub fn set_chunk_chars(&mut self, n: usize) -> Vec<Effect> {
        self.chunk_chars = n;
        self.finish(Vec::new())
    }

    pub fn chunk_chars(&self) -> usize {
        self.chunk_chars
    }

    /// Whether the first joined chunk of an item is one sentence (true, the
    /// default) or joins up to `chunk_chars` like the rest. Items already
    /// queued keep their chunks.
    pub fn set_quick_start(&mut self, on: bool) -> Vec<Effect> {
        self.quick_start = on;
        self.finish(Vec::new())
    }

    pub fn quick_start(&self) -> bool {
        self.quick_start
    }

    /// The current state, with the `seq` of the last emitted state.
    pub fn state(&self) -> State {
        self.shown.clone()
    }

    /// The item being read.
    pub fn current(&self) -> Option<&Item> {
        self.current.as_ref().map(|c| &c.item)
    }

    /// The items waiting after the current one, in order.
    pub fn queued(&self) -> impl Iterator<Item = &Item> {
        self.queue.iter()
    }

    // ---- transitions ----

    fn pause(&mut self, fx: &mut Vec<Effect>) {
        if self.current.is_none() || self.paused {
            return;
        }
        self.paused = true;
        if self.loaded.is_some() {
            fx.push(Effect::PauseOutput);
        }
    }

    fn resume(&mut self, fx: &mut Vec<Effect>) {
        self.paused = false;
        if self.loaded.is_some() {
            fx.push(Effect::ResumeOutput);
        } else {
            self.begin_chunk(fx);
        }
    }

    fn stop(&mut self, fx: &mut Vec<Effect>) {
        self.last = None;
        if self.current.is_none() && self.queue.is_empty() {
            return;
        }
        if self.current.is_some() {
            self.end_current(fx, ItemPhase::Skipped);
        }
        while let Some(dropped) = self.queue.pop_front() {
            self.forget(fx, dropped.id, ItemPhase::Skipped);
        }
        self.last = None;
        self.paused = false;
    }

    fn skip(&mut self, fx: &mut Vec<Effect>) {
        self.end_current(fx, ItemPhase::Skipped);
        self.start_next(fx);
    }

    /// Jump to chunk `to` of the current item, replacing what is loaded.
    fn move_to(&mut self, fx: &mut Vec<Effect>, to: usize) {
        self.stop_output(fx);
        if let Some(cur) = &mut self.current {
            cur.chunk = to;
        }
        self.begin_chunk(fx);
    }

    /// The loaded chunk ended (finished or failed): go to the next chunk, or
    /// end the item and start the next one.
    fn advance(&mut self, fx: &mut Vec<Effect>) {
        let Some(cur) = &mut self.current else {
            return;
        };
        if cur.chunk + 1 < cur.item.chunks.len() {
            cur.chunk += 1;
            self.begin_chunk(fx);
        } else {
            let phase = if cur.played_any {
                ItemPhase::Finished
            } else {
                ItemPhase::Failed
            };
            self.end_current(fx, phase);
            self.start_next(fx);
        }
    }

    /// Make the front of the queue current, or go idle (and unpaused).
    fn start_next(&mut self, fx: &mut Vec<Effect>) {
        match self.queue.pop_front() {
            Some(item) => {
                emit_item(fx, item.id, ItemPhase::Started);
                self.current = Some(Current {
                    item,
                    chunk: 0,
                    played_any: false,
                });
                self.begin_chunk(fx);
            }
            None => self.paused = false,
        }
    }

    /// Synthesize the current chunk and, unless paused, play it (`finish`
    /// prefetches the one after it). Nothing may be loaded in the output.
    fn begin_chunk(&mut self, fx: &mut Vec<Effect>) {
        let Some(cur) = &self.current else {
            return;
        };
        let (id, chunk) = (cur.item.id, cur.chunk);
        let text = cur.item.chunks[chunk].clone();
        self.request(fx, id, chunk, text);
        if self.paused {
            return;
        }
        self.gen += 1;
        self.loaded = Some(self.gen);
        fx.push(Effect::PlayChunk {
            item: id,
            chunk,
            gen: self.gen,
        });
    }

    /// Synthesize the `lookahead` chunks after the playing one, in play
    /// order: the rest of the current item, then the queued items.
    fn prefetch(&mut self, fx: &mut Vec<Effect>) {
        if self.loaded.is_none() {
            return;
        }
        let Some(cur) = &self.current else {
            return;
        };
        let ahead: Vec<(ItemId, usize, String)> = cur
            .item
            .chunks
            .iter()
            .enumerate()
            .skip(cur.chunk + 1)
            .map(|(i, t)| (cur.item.id, i, t.clone()))
            .chain(self.queue.iter().flat_map(|item| {
                item.chunks
                    .iter()
                    .enumerate()
                    .map(move |(i, t)| (item.id, i, t.clone()))
            }))
            .take(self.lookahead)
            .collect();
        for (id, chunk, text) in ahead {
            self.request(fx, id, chunk, text);
        }
    }

    fn request(&mut self, fx: &mut Vec<Effect>, item: ItemId, chunk: usize, text: String) {
        if self.requested.insert((item, chunk)) {
            fx.push(Effect::Synthesize { item, chunk, text });
        }
    }

    fn stop_output(&mut self, fx: &mut Vec<Effect>) {
        if self.loaded.take().is_some() {
            fx.push(Effect::StopOutput);
        }
    }

    fn end_current(&mut self, fx: &mut Vec<Effect>, phase: ItemPhase) {
        self.stop_output(fx);
        if let Some(cur) = self.current.take() {
            self.forget(fx, cur.item.id, phase);
            self.last = Some(cur.item);
        }
    }

    /// Report the final phase of an item and drop its synthesized chunks.
    fn forget(&mut self, fx: &mut Vec<Effect>, id: ItemId, phase: ItemPhase) {
        self.requested.retain(|(item, _)| *item != id);
        emit_item(fx, id, phase);
    }

    // ---- state ----

    fn snapshot(&self, seq: u64) -> State {
        State {
            seq,
            now_playing: self.current.as_ref().map(|c| NowPlaying {
                item_id: c.item.id,
                label: c.item.label.clone(),
                text: c.item.chunks[c.chunk].clone(),
                chunk: c.chunk,
                chunks: c.item.chunks.len(),
            }),
            queued: self.queue.len(),
            paused: self.paused,
            muted: self.muted,
            volume: self.volume,
            rate: self.rate,
            voice: self.voice.clone(),
        }
    }

    /// Prefetch for the new situation and append a state event if anything
    /// visible changed.
    fn finish(&mut self, mut fx: Vec<Effect>) -> Vec<Effect> {
        self.prefetch(&mut fx);
        let next = self.snapshot(self.shown.seq);
        if next != self.shown {
            let state = self.snapshot(self.shown.seq + 1);
            self.shown = state.clone();
            fx.push(Effect::Emit(Event::State(state)));
        }
        fx
    }
}

fn emit_item(fx: &mut Vec<Effect>, item_id: ItemId, phase: ItemPhase) {
    fx.push(Effect::Emit(Event::Item { item_id, phase }));
}
