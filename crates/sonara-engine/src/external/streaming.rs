//! A chunk whose audio plays as it comes (#235): Gemini's events
//! (`Adapter::stream_request`, read by `sse::start`) or, in send mode
//! `message`, a raw PCM body read as it arrives (`Adapter::bytes_request`,
//! `sse::start_bytes`: ElevenLabs, OpenAI `pcm`, Cartesia). The first
//! audio decides. When it comes within the profile's `first_audio_ms`
//! (default 12 s), `synthesize` returns at once and the rest follows on the
//! `PcmStream`, which the reader plays while it arrives
//! (`Engine::streams`). When it does not, the chunk is a `timeout` failure
//! like any other: the fallback reads it, with the cue once per episode,
//! and the breaker counts it. The whole answer may take the profile's
//! `timeout_ms`; past it, or when the answer breaks off after audio came,
//! the chunk ends with the audio received (spec 13.2: audio received is
//! used) and the notice says why.
use super::adapter::{Adapter, StreamPiece};
use super::cache::CueCache;
use super::error::ExtError;
use super::hold::{self, Hold, Scope};
use super::keys::Secret;
use super::sse::{self, Event};
use super::worker::CancelToken;
use super::{External, Notice, NoticeFn, RETRY_AFTER_MAX};
use crate::{EngineId, Error, PcmChunk, PcmStream, Reason, Result};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a wait looks at the cancel generation.
const POLL: Duration = Duration::from_millis(10);

/// How the answer comes: server-sent events (the adapter decodes each),
/// or raw 16-bit mono PCM at this rate.
#[derive(Clone, Copy)]
enum Wire {
    Sse,
    Raw(u32),
}

/// The audio of one event of `wire`; `carry` holds a sample cut in half
/// between two events.
fn decode(
    adapter: &Arc<dyn Adapter>,
    wire: Wire,
    carry: &mut Vec<u8>,
    event: Event,
    label: &str,
) -> std::result::Result<StreamPiece, ExtError> {
    match (wire, event) {
        (Wire::Sse, Event::Data(d)) => adapter.stream_piece(carry, &d, label),
        (Wire::Raw(rate), Event::Bytes(b)) => {
            carry.extend_from_slice(&b);
            let (pairs, _) = carry.as_chunks::<2>();
            let samples: Vec<i16> = pairs.iter().map(|p| i16::from_le_bytes(*p)).collect();
            carry.drain(..samples.len() * 2);
            Ok(StreamPiece {
                audio: Some(PcmChunk {
                    samples,
                    sample_rate: rate,
                    channels: 1,
                }),
                note: None,
            })
        }
        (_, Event::Whole(reply)) => adapter.audio(&reply, label).map(|pcm| StreamPiece {
            audio: Some(pcm),
            note: None,
        }),
        _ => Ok(StreamPiece::default()),
    }
}

/// How a streamed chunk began.
pub(super) enum Begun {
    /// Audio is coming: the first piece and the rest.
    Playing(PcmStream),
    /// Not streamed (the adapter has no stream, now or for this text):
    /// the whole-answer path applies.
    Whole,
    /// No audio came (or it came too late): a failure of this chunk.
    Failed(ExtError),
}

/// What the wait for the first audio saw.
enum First {
    /// The first audio, the rest of the stream and the half sample it
    /// ended with, if any.
    Audio(PcmChunk, Receiver<Event>, Vec<u8>),
    Failed(ExtError),
}

impl External {
    /// The first audio of `text` from the stream, within `first_audio_ms`.
    /// `Err(Cancelled)` when the reader cancelled meanwhile.
    pub(super) fn begin_stream(
        &self,
        gen: u64,
        epoch: u64,
        text: &str,
        voice: &str,
        rate: u32,
        key: Option<&Secret>,
    ) -> Result<Begun> {
        let super::Backend::Http(adapter) = &self.backend else {
            return Ok(Begun::Whole);
        };
        // One request per chunk: a text the adapter would split is read
        // whole, part by part.
        if super::split::split(text, adapter.input_limit()).len() != 1 {
            return Ok(Begun::Whole);
        }
        // Gemini's events; else, for a whole message, a raw body as it
        // comes; else the whole answer.
        let raw = !adapter.streams() && self.whole_messages();
        let wire = match (raw, adapter.requested_rate()) {
            (false, _) if adapter.streams() => Wire::Sse,
            (true, Some(r)) => Wire::Raw(r),
            _ => return Ok(Begun::Whole),
        };
        let make = |adapter: &Arc<dyn Adapter>| {
            if raw {
                adapter.bytes_request(text, voice, rate, key)
            } else {
                adapter.stream_request(text, voice, rate, key)
            }
        };
        let Some(mut req) = make(adapter) else {
            return Ok(Begun::Whole);
        };
        let first_audio = Duration::from_millis(self.profile.first_audio_ms());
        let total = Duration::from_millis(self.profile.timeout_ms());
        let (mut retried, mut adapted) = (false, 0);
        loop {
            let started = Instant::now();
            let rx = match wire {
                Wire::Sse => sse::start(self.agent.clone(), req.clone(), self.host.clone()),
                Wire::Raw(_) => {
                    sse::start_bytes(self.agent.clone(), req.clone(), self.host.clone())
                }
            };
            match self.first_audio(adapter, wire, gen, rx, first_audio, voice)? {
                First::Audio(pcm, rx, carry) => {
                    return Ok(Begun::Playing(Box::new(Rest {
                        first: Some(pcm),
                        rx: Some(rx),
                        wire,
                        carry,
                        deadline: started + total,
                        adapter: adapter.clone(),
                        label: self.label.clone(),
                        cancel: self.cancel.clone(),
                        gen,
                        hold: self.hold.clone(),
                        epoch,
                        scope: hold::scope(),
                        got: Vec::new(),
                        cache: self.cache.clone(),
                        cache_key: (voice.to_string(), rate, text.to_string()),
                        message: self.whole_messages(),
                        notice: self.notice.clone(),
                        engine: self.id,
                        done: false,
                    })));
                }
                First::Failed(e)
                    if !retried
                        && e.reason != Reason::Quota
                        && matches!(e.status, Some(429) | Some(503))
                        && e.retry_after.is_some_and(|d| d <= RETRY_AFTER_MAX) =>
                {
                    retried = true;
                    self.cancel
                        .sleep(gen, e.retry_after.unwrap_or_default())
                        .map_err(|_| Error::Cancelled)?;
                }
                First::Failed(e) if adapted < super::MAX_ADAPTS + 1 && adapter.adapt(&req, &e) => {
                    adapted += 1;
                    // A refused stream: the whole answer from now on.
                    match make(adapter) {
                        Some(r) if raw || adapter.streams() => req = r,
                        _ => return Ok(Begun::Whole),
                    }
                }
                First::Failed(e) => return Ok(Begun::Failed(e)),
            }
        }
    }

    /// Wait for the first audio of `rx`, at most `limit`.
    fn first_audio(
        &self,
        adapter: &Arc<dyn Adapter>,
        wire: Wire,
        gen: u64,
        rx: Receiver<Event>,
        limit: Duration,
        voice: &str,
    ) -> Result<First> {
        let end = Instant::now() + limit;
        let mut note: Option<String> = None;
        let mut carry = Vec::new();
        loop {
            if self.cancel.generation() != gen {
                return Err(Error::Cancelled);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(First::Failed(ExtError::new(
                    Reason::Timeout,
                    format!(
                        "{} sent no audio within {} s",
                        self.label,
                        limit.as_millis().div_ceil(1000)
                    ),
                )));
            }
            match rx.recv_timeout(left.min(POLL)) {
                Ok(ev @ (Event::Data(_) | Event::Bytes(_) | Event::Whole(_))) => {
                    match decode(adapter, wire, &mut carry, ev, &self.label) {
                        Ok(StreamPiece {
                            audio: Some(pcm), ..
                        }) if !pcm.samples.is_empty() => return Ok(First::Audio(pcm, rx, carry)),
                        Ok(p) => note = p.note.or(note),
                        Err(e) => return Ok(First::Failed(e)),
                    }
                }
                Ok(Event::Refused(reply)) => {
                    return Ok(First::Failed(adapter.map_error(
                        &reply,
                        voice,
                        self.voice_listed(voice),
                    )))
                }
                Ok(Event::Failed(e)) => return Ok(First::Failed(e)),
                Ok(Event::End) | Err(RecvTimeoutError::Disconnected) => {
                    return Ok(First::Failed(ExtError::new(
                        Reason::Server,
                        format!(
                            "{} sent no audio for this text (reason: {})",
                            self.label,
                            note.as_deref().unwrap_or("none given")
                        ),
                    )))
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// The whole audio of `text` from the stream, for `test` (no fallback).
    pub(super) fn stream_whole(
        &self,
        gen: u64,
        text: &str,
        voice: &str,
        rate: u32,
        key: Option<&Secret>,
    ) -> Result<Option<std::result::Result<Vec<PcmChunk>, ExtError>>> {
        match self.begin_stream(gen, self.hold_epoch(), text, voice, rate, key)? {
            Begun::Whole => Ok(None),
            Begun::Failed(e) => Ok(Some(Err(e))),
            Begun::Playing(s) => Ok(Some(Ok(s.collect::<Result<Vec<_>>>()?))),
        }
    }
}

/// The rest of a streamed chunk, after its first audio.
struct Rest {
    first: Option<PcmChunk>,
    rx: Option<Receiver<Event>>,
    wire: Wire,
    /// A half sample from the last event (`Adapter::stream_piece`).
    carry: Vec<u8>,
    deadline: Instant,
    adapter: Arc<dyn Adapter>,
    label: String,
    cancel: Arc<CancelToken>,
    gen: u64,
    hold: Option<Arc<Hold>>,
    epoch: u64,
    scope: Scope,
    /// Everything played so far, for the cue cache at the end.
    got: Vec<PcmChunk>,
    cache: Arc<CueCache>,
    cache_key: (String, u32, String),
    /// A whole message (send mode `message`): kept for a replay.
    message: bool,
    notice: Option<NoticeFn>,
    engine: EngineId,
    done: bool,
}

impl Rest {
    /// The stream ended early: the audio received stays, and the log says
    /// why (no fallback: part of the text was already spoken).
    fn cut(&mut self, reason: Reason, message: String) -> Option<Result<PcmChunk>> {
        self.done = true;
        self.rx = None;
        if let Some(n) = &self.notice {
            n(Notice {
                engine: self.engine,
                reason: Some(reason),
                status: None,
                message,
                fallback: None,
            });
        }
        None
    }

    /// A cancel came from the hold (Sonara muted meanwhile), not from the
    /// reader: the chunk ends with what was spoken, it is not dropped.
    fn cut_by_hold(&self) -> bool {
        self.scope == Scope::Normal
            && self
                .hold
                .as_ref()
                .is_some_and(|h| h.is_held() || h.epoch() != self.epoch)
    }

    /// Whether the audio is kept for the cache (a cue, or a whole message).
    fn keeps(&self) -> bool {
        self.message || CueCache::cacheable(&self.cache_key.2)
    }

    fn finish(&mut self) -> Option<Result<PcmChunk>> {
        self.done = true;
        self.rx = None;
        let (voice, rate, text) = &self.cache_key;
        self.cache.keep(
            voice,
            *rate,
            text,
            std::mem::take(&mut self.got),
            self.message,
        );
        None
    }
}

impl Iterator for Rest {
    type Item = Result<PcmChunk>;

    fn next(&mut self) -> Option<Result<PcmChunk>> {
        if self.done {
            return None;
        }
        if let Some(pcm) = self.first.take() {
            if self.keeps() {
                self.got.push(pcm.clone());
            }
            return Some(Ok(pcm));
        }
        loop {
            if self.cancel.generation() != self.gen {
                self.done = true;
                self.rx = None;
                if self.cut_by_hold() {
                    return None;
                }
                return Some(Err(Error::Cancelled));
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let m = format!("{} did not finish the answer in time", self.label);
                return self.cut(Reason::Timeout, m);
            }
            let rx = self.rx.as_ref()?;
            match rx.recv_timeout(left.min(POLL)) {
                Ok(ev @ (Event::Data(_) | Event::Bytes(_) | Event::Whole(_))) => {
                    match decode(&self.adapter, self.wire, &mut self.carry, ev, &self.label) {
                        Ok(StreamPiece {
                            audio: Some(pcm), ..
                        }) if !pcm.samples.is_empty() => {
                            if self.keeps() {
                                self.got.push(pcm.clone());
                            }
                            return Some(Ok(pcm));
                        }
                        Ok(_) => {}
                        Err(e) => return self.cut(e.reason, e.message),
                    }
                }
                Ok(Event::End) | Err(RecvTimeoutError::Disconnected) => return self.finish(),
                Ok(Event::Failed(e)) => return self.cut(e.reason, e.message),
                Ok(Event::Refused(_)) => {
                    let m = format!("{} refused the rest of the answer", self.label);
                    return self.cut(Reason::Server, m);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
