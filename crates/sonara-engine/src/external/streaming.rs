//! A chunk whose audio plays as it comes (#235): Gemini's events
//! (`Adapter::stream_request`, read by `sse::start`) or, in send mode
//! `message`, a raw PCM body read as it arrives (`Adapter::bytes_request`,
//! `sse::start_bytes`: ElevenLabs, OpenAI `pcm`, Cartesia). The first
//! audio decides. When it comes within the profile's `first_audio_ms`
//! (default 12 s), `synthesize` returns at once and the rest follows on the
//! `PcmStream`, which the reader plays while it arrives
//! (`Engine::streams`). When it does not, the chunk is a `timeout` failure
//! like any other: the fallback reads it, with the cue once per episode,
//! and the breaker counts it. After the first audio the same limit is a
//! stall limit: no audio for `first_audio_ms` ends the answer. Its length
//! is not the limit (review of #236): the whole answer may take
//! `Profile::answer_ms` (`timeout_ms` plus the time its text takes to
//! speak), which also bounds the request thread (`Within`).
//!
//! An answer that ends early after audio came (a stall, a broken
//! connection, `answer_ms`) keeps the audio received (spec 13.2). In send
//! mode `message` the rest of the text is then read with the fallback,
//! the cue first once per episode, from the start of the sentence the
//! audio had reached (`rest_of`: an estimate from the seconds played at
//! `CHARS_PER_SECOND`, set low so a sentence may be read twice but none
//! is dropped); the notice names the fallback. A mute (the hold) during a
//! message reads the rest locally the same way, with no cue and no notice
//! (a mute is no failure). In send mode `sentence` the chunk ends with
//! what was spoken, as before.
use super::adapter::{Adapter, StreamPiece, Within};
use super::cache::CueCache;
use super::error::cue_text;
use super::error::ExtError;
use super::health::Health;
use super::hold::{self, Hold, Scope};
use super::keys::Secret;
use super::sse::{self, Event};
use super::worker::CancelToken;
use super::{External, Notice, NoticeFn, RETRY_AFTER_MAX};
use crate::{Engine, EngineId, Error, PcmChunk, PcmStream, Reason, Result};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a wait looks at the cancel generation.
const POLL: Duration = Duration::from_millis(10);

/// The speaking rate assumed to find where a cut answer stopped in its
/// text: lower than most voices (about 15 characters a second), so the
/// estimate falls behind the real place, never ahead of it.
pub const CHARS_PER_SECOND: f64 = 12.0;

/// The text still to read after `secs` seconds of `text` were spoken: from
/// the start of the sentence the estimated place is in (`CHARS_PER_SECOND`),
/// trimmed; empty when the estimate is past the end.
pub fn rest_of(text: &str, secs: f64) -> &str {
    let reached = (secs.max(0.0) * CHARS_PER_SECOND) as usize;
    let Some((at, _)) = text.char_indices().nth(reached) else {
        return "";
    };
    // The last sentence start at or before `at`: after a `.`, `!`, `?` or
    // a line break followed by white space.
    let (mut start, mut ended, mut gap) = (0, false, false);
    for (i, c) in text.char_indices() {
        if i > at {
            break;
        }
        if c == '\n' || (ended && c.is_whitespace()) {
            gap = true;
        } else if !c.is_whitespace() {
            if gap {
                start = i;
            }
            gap = false;
            ended = matches!(c, '.' | '!' | '?');
        }
    }
    text[start..].trim()
}

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
        let total = Duration::from_millis(self.profile.answer_ms(text.chars().count()));
        // The headers within the wait for the first audio (a provider that
        // does not answer is dropped then), the body within `answer_ms`.
        let within = Some(Within {
            response: first_audio,
            body: total,
        });
        let (mut retried, mut adapted) = (false, 0);
        loop {
            let started = Instant::now();
            let rx = match wire {
                Wire::Sse => sse::start(self.agent.clone(), req.clone(), self.host.clone(), within),
                Wire::Raw(_) => {
                    sse::start_bytes(self.agent.clone(), req.clone(), self.host.clone(), within)
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
                        idle: first_audio,
                        last: Instant::now(),
                        played: 0.0,
                        tail: None,
                        fallback: self.fallback.clone(),
                        fallback_voice: self.fallback_voice.clone(),
                        health: self.health.clone(),
                        rate,
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
    /// The longest wait for the next audio (`first_audio_ms`).
    idle: Duration,
    /// When the last audio came.
    last: Instant,
    /// Seconds of audio played so far (`rest_of`).
    played: f64,
    /// The rest of a message read with the fallback after a cut.
    tail: Option<PcmStream>,
    fallback: Option<Arc<dyn Engine>>,
    fallback_voice: String,
    health: Arc<Health>,
    rate: u32,
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
    /// The stream ended early: the audio received stays, the log says why,
    /// and in a message the rest of the text is read with the fallback
    /// (module docs).
    fn cut(&mut self, reason: Reason, message: String) -> Option<Result<PcmChunk>> {
        self.done = true;
        self.rx = None;
        let rest = self.rest_text();
        let fallback = match (&rest, &self.fallback) {
            (Some(_), Some(f)) => Some(f.id()),
            _ => None,
        };
        if let Some(n) = &self.notice {
            n(Notice {
                engine: self.engine,
                reason: Some(reason),
                status: None,
                message,
                fallback,
            });
        }
        let rest = rest?;
        let fb = self.fallback.clone()?;
        let mut cue: Vec<Result<PcmChunk>> = Vec::new();
        if self.health.take_cue() {
            let line = cue_text(reason, &self.label);
            match fb.synthesize(&line, &self.fallback_voice, self.rate) {
                Ok(s) => cue.extend(s),
                Err(e) => return Some(Err(e)),
            }
        }
        match fb.synthesize(&rest, &self.fallback_voice, self.rate) {
            Ok(s) => self.tail = Some(Box::new(cue.into_iter().chain(s))),
            Err(e) => return Some(Err(e)),
        }
        self.next_of_tail()
    }

    /// A mute (the hold) cut a message: the rest is read locally, no cue,
    /// no notice (module docs). Elsewhere the chunk ends with what was
    /// spoken.
    fn cut_quietly(&mut self) -> Option<Result<PcmChunk>> {
        let rest = self.rest_text()?;
        let fb = self.fallback.clone()?;
        match fb.synthesize(&rest, &self.fallback_voice, self.rate) {
            Ok(s) => self.tail = Some(s),
            Err(e) => return Some(Err(e)),
        }
        self.next_of_tail()
    }

    /// The text still to read after a cut of a message (`None`: not a
    /// message, or nothing left).
    fn rest_text(&self) -> Option<String> {
        if !self.message {
            return None;
        }
        let rest = rest_of(&self.cache_key.2, self.played);
        (!rest.is_empty()).then(|| rest.to_string())
    }

    fn next_of_tail(&mut self) -> Option<Result<PcmChunk>> {
        let next = self.tail.as_mut()?.next();
        if next.is_none() {
            self.tail = None;
        }
        next
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

    fn played(&mut self, pcm: &PcmChunk) {
        let per_second = pcm.sample_rate.max(1) as f64 * pcm.channels.max(1) as f64;
        self.played += pcm.samples.len() as f64 / per_second;
        self.last = Instant::now();
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
        if self.tail.is_some() {
            return self.next_of_tail();
        }
        if self.done {
            return None;
        }
        if let Some(pcm) = self.first.take() {
            if self.keeps() {
                self.got.push(pcm.clone());
            }
            self.played(&pcm);
            return Some(Ok(pcm));
        }
        loop {
            if self.cancel.generation() != self.gen {
                self.done = true;
                self.rx = None;
                if self.cut_by_hold() {
                    return self.cut_quietly();
                }
                return Some(Err(Error::Cancelled));
            }
            let now = Instant::now();
            let left = self.deadline.saturating_duration_since(now);
            if left.is_zero() {
                let m = format!("{} did not finish the answer in time", self.label);
                return self.cut(Reason::Timeout, m);
            }
            let quiet = (self.last + self.idle).saturating_duration_since(now);
            if quiet.is_zero() {
                let m = format!(
                    "{} sent no audio for {} s",
                    self.label,
                    self.idle.as_millis().div_ceil(1000)
                );
                return self.cut(Reason::Timeout, m);
            }
            let rx = self.rx.as_ref()?;
            match rx.recv_timeout(left.min(quiet).min(POLL)) {
                Ok(ev @ (Event::Data(_) | Event::Bytes(_) | Event::Whole(_))) => {
                    match decode(&self.adapter, self.wire, &mut self.carry, ev, &self.label) {
                        Ok(StreamPiece {
                            audio: Some(pcm), ..
                        }) if !pcm.samples.is_empty() => {
                            if self.keeps() {
                                self.got.push(pcm.clone());
                            }
                            self.played(&pcm);
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

#[cfg(test)]
mod tests {
    use super::rest_of;

    #[test]
    fn the_rest_starts_at_the_sentence_the_audio_reached() {
        let t = "One two three. Four five six! Seven?\n\nEight nine.";
        assert_eq!(rest_of(t, 0.0), t);
        // 12 characters: still in the first sentence.
        assert_eq!(rest_of(t, 1.0), t);
        // 24 characters: in "Four five six!", which is read again.
        assert_eq!(rest_of(t, 2.0), "Four five six! Seven?\n\nEight nine.");
        // 42 characters: in "Eight nine."
        assert_eq!(rest_of(t, 3.5), "Eight nine.");
        assert_eq!(rest_of(t, 60.0), "", "past the end: nothing left");
        // A decimal point is no sentence end.
        let d = "It is 3.5 times faster than before.";
        assert_eq!(rest_of(d, 2.0), d);
    }
}
