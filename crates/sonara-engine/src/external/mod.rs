//! External engines (feature `external`; spec
//! `docs/plans/2026-10-04-external-engines-spec.md`): a profile the user
//! added at run time, spoken to over HTTP, as an `Engine` next to Kokoro and
//! OneCore. Licence class `External`, so a host must allow it.
//!
//! - **Never silent** (spec 8): any failure of the provider speaks that
//!   chunk with the fallback engine (in `sonarad`: Kokoro, which falls back
//!   to OneCore), with a short spoken cue once per episode saying why. Two
//!   transient failures in a row open a breaker (30 s doubling to 300 s), so
//!   a provider that is down costs no waiting; failures the user must fix
//!   (no key, a refused key, a bad voice) block it until they change.
//! - **No retry loop**: a chunk goes to the provider once; only a 429 or
//!   503 with `Retry-After` of at most 1.5 s is tried once more.
//! - **Cancel** ends a synthesis at once: the request runs on its own
//!   thread (`worker`), the wait is what `cancel` ends.
//! - **Keys** are read per request (`keys::KeyResolver`), so a key set with
//!   `engine_key` applies to the next chunk. A key goes only in the
//!   provider's auth header, over https or to a loopback host.
//! - **Muted** (`hold`, #227): while the host's `Hold` is held, nothing is
//!   sent: chunks are spoken with the fallback and voice lists are not
//!   fetched; raising it ends a request in flight. `test` and a synthesis
//!   inside `hold::explicit` (the user's own actions) still reach the
//!   provider.
pub mod adapter;
pub mod audio;
pub mod azure;
pub mod cache;
pub mod cartesia;
pub mod command;
pub mod deepgram;
pub mod elevenlabs;
pub mod error;
pub mod gemini;
pub mod google;
pub mod health;
pub mod hold;
pub mod keys;
pub mod openai;
pub mod profile;
pub mod rate;
pub mod split;
pub mod worker;

use crate::{
    Engine, EngineId, EngineStatus, Error, LicenseClass, PcmChunk, PcmStream, Readiness, Reason,
    Result, Voice,
};
use adapter::{execute, Adapter, HttpRequest, VoiceSource, MAX_AUDIO_BODY, MAX_LIST_BODY};
use cache::CueCache;
use error::{cue_text, ExtError};
use health::{Clock, Health, View};
use hold::{Hold, Scope};
use keys::{KeyResolver, Secret};
use profile::{Kind, Profile, ProfileError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use worker::CancelToken;

/// A `Retry-After` up to this long is waited for once.
pub const RETRY_AFTER_MAX: Duration = Duration::from_millis(1500);
/// The voice list is fetched again after this long.
pub const VOICES_TTL: Duration = Duration::from_secs(600);
/// After a failed voice-list fetch the list is not stale for this long, so
/// a host that asks often does not repeat a refused key or a dead host.
pub const VOICES_RETRY: Duration = Duration::from_secs(60);
/// A voice-list request may take this long.
pub const VOICES_TIMEOUT: Duration = Duration::from_secs(10);
/// At most this many pages of a paged voice list are read.
pub const VOICES_MAX_PAGES: usize = 50;

/// What happened, for the host's log (spec 8.3). `reason: None` is a
/// recovery after failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub engine: EngineId,
    pub reason: Option<Reason>,
    pub status: Option<u16>,
    pub message: String,
    /// The engine that spoke instead (none: the chunk was skipped).
    pub fallback: Option<EngineId>,
}

pub type NoticeFn = Arc<dyn Fn(Notice) + Send + Sync>;

/// How to build an `External`.
pub struct ExternalConfig {
    pub profile: Profile,
    pub keys: KeyResolver,
    /// What speaks when the provider cannot (sonarad: Kokoro).
    pub fallback: Option<Arc<dyn Engine>>,
    /// The fallback's voice (sonarad: `af_sarah` when it offers it).
    pub fallback_voice: String,
    pub notice: Option<NoticeFn>,
    pub clock: Clock,
    /// Tests may inject an agent; default: `http::agent` with the profile's
    /// timeouts.
    pub agent: Option<ureq::Agent>,
    /// The host's mute (`hold`): while held, nothing is sent.
    pub hold: Option<Arc<Hold>>,
}

impl ExternalConfig {
    pub fn new(profile: Profile, keys: KeyResolver) -> ExternalConfig {
        ExternalConfig {
            profile,
            keys,
            fallback: None,
            fallback_voice: String::new(),
            notice: None,
            clock: Arc::new(Instant::now),
            agent: None,
            hold: None,
        }
    }
}

/// The result of `External::test`.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub voice: String,
    pub ms: u64,
    pub pcm: Vec<PcmChunk>,
}

/// How one request is made (spec 3): an HTTP adapter, or a local program.
enum Backend {
    Http(Arc<dyn Adapter>),
    Command(Arc<command::Command>),
}

impl Backend {
    fn input_limit(&self) -> split::Limit {
        match self {
            Backend::Http(a) => a.input_limit(),
            Backend::Command(c) => c.input_limit(),
        }
    }
}

pub struct External {
    id: EngineId,
    profile: Profile,
    backend: Backend,
    keys: KeyResolver,
    fallback: Option<Arc<dyn Engine>>,
    fallback_voice: String,
    notice: Option<NoticeFn>,
    health: Health,
    cache: CueCache,
    cancel: Arc<CancelToken>,
    /// The hold epoch and cancel generation at `begin`, taken by the next
    /// `synthesize`.
    begun: Mutex<Option<(u64, u64)>>,
    agent: ureq::Agent,
    voices_agent: ureq::Agent,
    label: String,
    host: String,
    voices: Mutex<Option<(Instant, Vec<Voice>)>>,
    /// When the last voice-list fetch failed (cleared by a success).
    voices_failed: Mutex<Option<Instant>>,
    clock: Clock,
    hold: Option<Arc<Hold>>,
}

fn backend_for(p: &Profile) -> Backend {
    match p.kind {
        Kind::OpenAiCompatible => Backend::Http(Arc::new(openai::OpenAi::new(p))),
        Kind::ElevenLabs => Backend::Http(Arc::new(elevenlabs::ElevenLabs::new(p))),
        Kind::Azure => Backend::Http(Arc::new(azure::Azure::new(p))),
        Kind::Google => Backend::Http(Arc::new(google::Google::new(p))),
        Kind::Gemini => Backend::Http(Arc::new(gemini::Gemini::new(p))),
        Kind::Cartesia => Backend::Http(Arc::new(cartesia::Cartesia::new(p))),
        Kind::Deepgram => Backend::Http(Arc::new(deepgram::Deepgram::new(p))),
        Kind::Command => Backend::Command(Arc::new(command::Command::new(p))),
    }
}

impl External {
    /// Fails only on an invalid profile or a kind this build lacks.
    pub fn new(config: ExternalConfig) -> std::result::Result<External, ProfileError> {
        let p = config.profile;
        p.validate()?;
        let backend = backend_for(&p);
        let timeout = Duration::from_millis(p.timeout_ms());
        // No redirects, and no proxy for a loopback server (spec 6.4).
        let direct = p.parsed_url().is_some_and(|u| u.is_loopback());
        let agent = config.agent.clone().unwrap_or_else(|| {
            crate::http::provider_agent(
                crate::http::Timeouts {
                    connect: Duration::from_secs(5),
                    recv_response: timeout,
                    // A body that stalls (Wi-Fi gone after the headers) falls
                    // back after the same wait, not 30 s more.
                    recv_body: timeout,
                },
                direct,
            )
        });
        let voices_agent = config.agent.unwrap_or_else(|| {
            crate::http::provider_agent(
                crate::http::Timeouts {
                    connect: Duration::from_secs(5),
                    recv_response: VOICES_TIMEOUT,
                    recv_body: VOICES_TIMEOUT,
                },
                direct,
            )
        });
        let cancel = Arc::new(CancelToken::new());
        if let Some(h) = &config.hold {
            h.watch(&cancel);
        }
        Ok(External {
            id: EngineId::intern(&p.id),
            label: p.display_label(),
            host: p.host(),
            backend,
            keys: config.keys,
            fallback: config.fallback,
            fallback_voice: config.fallback_voice,
            notice: config.notice,
            health: Health::new(config.clock.clone()),
            cache: CueCache::new(),
            cancel,
            begun: Mutex::new(None),
            agent,
            voices_agent,
            voices: Mutex::new(None),
            voices_failed: Mutex::new(None),
            clock: config.clock,
            hold: config.hold,
            profile: p,
        })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Whether a key resolves now.
    pub fn key_present(&self) -> bool {
        self.keys.present(&self.profile)
    }

    /// A new key was stored or removed: re-check the `no_key`/`auth` block.
    pub fn key_changed(&self) {
        self.health.clear_key_block();
    }

    /// Forget failures and cached audio (a profile change).
    pub fn reset(&self) {
        self.health.clear();
        self.cache.clear();
    }

    /// Whether nothing may be sent now (`hold`): the host is muted (unless
    /// this thread runs an explicit action), or this thread speaks a local
    /// cue.
    pub fn held(&self) -> bool {
        match hold::scope() {
            Scope::Explicit => false,
            Scope::Local => true,
            Scope::Normal => self.hold.as_ref().is_some_and(|h| h.is_held()),
        }
    }

    /// The hold's epoch now (0 without a hold).
    fn hold_epoch(&self) -> u64 {
        self.hold.as_ref().map_or(0, |h| h.epoch())
    }

    /// The hold epoch, then the cancel generation: in this order, a hold
    /// raised after the generation was read is either seen by the `held`
    /// check that follows or has moved the generation, so the request is
    /// never sent (`Hold::set` holds before it cancels).
    fn mark(&self) -> (u64, u64) {
        let epoch = self.hold_epoch();
        (epoch, self.cancel.generation())
    }

    /// A cancel seen since `epoch` came from the hold (now held, or held
    /// and lifted again meanwhile), not from the reader: the chunk is
    /// spoken locally rather than dropped. An explicit action is cut like
    /// any other cancel.
    fn cut_by_hold(&self, epoch: u64) -> bool {
        self.held() || (hold::scope() == Scope::Normal && self.hold_epoch() != epoch)
    }

    /// Speak `text` with the fallback while held: not a failure, so no cue,
    /// no notice and no change of health.
    fn quiet(&self, text: &str, rate: u32) -> Result<PcmStream> {
        let Some(fb) = self.fallback.clone() else {
            return Err(Error::External {
                reason: Reason::BadConfig,
                message: format!(
                    "{} is not used while Sonara is muted, and there is no built-in voice",
                    self.label
                ),
            });
        };
        let pcm = fb
            .synthesize(text, &self.fallback_voice, rate)?
            .collect::<Result<Vec<_>>>()?;
        Ok(Box::new(pcm.into_iter().map(Ok)))
    }

    fn voice_for(&self, voice: &str) -> std::result::Result<String, ExtError> {
        let v = if voice.is_empty() {
            self.profile.effective_voice().unwrap_or_default()
        } else {
            voice.to_string()
        };
        // A program's voice is optional (`{voice}` may be unused).
        if v.is_empty() && self.profile.kind != Kind::Command {
            return Err(ExtError::new(
                Reason::BadConfig,
                format!("{} has no voice set", self.label),
            ));
        }
        Ok(v)
    }

    fn key(&self) -> std::result::Result<Option<Secret>, ExtError> {
        let key = self.keys.resolve(&self.profile)?;
        if key.is_none() && self.profile.needs_key() {
            return Err(ExtError::new(
                Reason::NoKey,
                format!("{} has no key", self.label),
            ));
        }
        Ok(key)
    }

    fn notify(&self, reason: Option<Reason>, status: Option<u16>, message: String) {
        if let Some(n) = &self.notice {
            n(Notice {
                engine: self.id,
                reason,
                status,
                message,
                fallback: self.fallback.as_ref().map(|f| f.id()),
            });
        }
    }

    /// One request for one part of a chunk.
    fn request(
        &self,
        gen: u64,
        text: &str,
        voice: &str,
        rate: u32,
        key: Option<&Secret>,
    ) -> Result<std::result::Result<PcmChunk, ExtError>> {
        match &self.backend {
            Backend::Http(adapter) => self.http_request(adapter, gen, text, voice, rate, key),
            Backend::Command(c) => {
                // The program runs on the request thread, which kills it on
                // a cancel (the wait here ends at once either way).
                let (c, token) = (c.clone(), self.cancel.clone());
                let (text, voice, key) = (text.to_string(), voice.to_string(), key.cloned());
                self.cancel
                    .run(gen, move || {
                        c.run(&text, &voice, rate, key.as_ref(), &|| {
                            token.generation() != gen
                        })
                    })
                    .map_err(|_| Error::Cancelled)
            }
        }
    }

    /// One provider request, with the one `Retry-After` retry and the one
    /// retry after the adapter adapted to a refusal (Deepgram's `speed`).
    fn http_request(
        &self,
        adapter: &Arc<dyn Adapter>,
        gen: u64,
        text: &str,
        voice: &str,
        rate: u32,
        key: Option<&Secret>,
    ) -> Result<std::result::Result<PcmChunk, ExtError>> {
        let mut req = adapter.synth_request(text, voice, rate, key);
        let (mut retried, mut adapted) = (false, false);
        loop {
            let (agent, r, host) = (self.agent.clone(), req.clone(), self.host.clone());
            let reply = self
                .cancel
                .run(gen, move || execute(&agent, &r, MAX_AUDIO_BODY, &host))
                .map_err(|_| Error::Cancelled)?;
            let outcome = match reply {
                Err(e) => Err(e),
                Ok(r) if r.ok() => adapter.audio(&r, &self.label),
                Ok(r) => Err(adapter.map_error(&r, voice, self.voice_listed(voice))),
            };
            match outcome {
                Err(e)
                    if !retried
                        && matches!(e.status, Some(429) | Some(503))
                        && e.retry_after.is_some_and(|d| d <= RETRY_AFTER_MAX) =>
                {
                    retried = true;
                    self.cancel
                        .sleep(gen, e.retry_after.unwrap_or_default())
                        .map_err(|_| Error::Cancelled)?;
                }
                Err(e) if !adapted && adapter.adapt(&req, &e) => {
                    adapted = true;
                    req = adapter.synth_request(text, voice, rate, key);
                }
                other => return Ok(other),
            }
        }
    }

    /// Every part of `text`, in order, from the provider.
    fn provider(
        &self,
        gen: u64,
        text: &str,
        voice: &str,
        rate: u32,
        key: Option<&Secret>,
    ) -> Result<std::result::Result<Vec<PcmChunk>, ExtError>> {
        let mut out = Vec::new();
        for part in split::split(text, self.backend.input_limit()) {
            match self.request(gen, &part, voice, rate, key)? {
                Ok(pcm) => out.push(pcm),
                Err(e) => return Ok(Err(e)),
            }
        }
        Ok(Ok(out))
    }

    /// Speak `text` with the fallback, the cue first once per episode.
    fn fallback(&self, text: &str, rate: u32, e: ExtError) -> Result<PcmStream> {
        let Some(fb) = self.fallback.clone() else {
            return Err(e.into_engine_error());
        };
        let mut pcm: Vec<PcmChunk> = Vec::new();
        if self.health.take_cue() {
            let cue = cue_text(e.reason, &self.label);
            pcm.extend(
                fb.synthesize(&cue, &self.fallback_voice, rate)?
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        pcm.extend(
            fb.synthesize(text, &self.fallback_voice, rate)?
                .collect::<Result<Vec<_>>>()?,
        );
        Ok(Box::new(pcm.into_iter().map(Ok)))
    }

    /// One synthesis with no fallback and no cache, for `engine_test`. A
    /// success clears the blocked state and the breaker.
    pub fn test(&self, text: &str, voice: &str, rate: u32) -> Result<TestResult> {
        let gen = self.cancel.generation();
        let voice = self.voice_for(voice).map_err(ExtError::into_engine_error)?;
        let key = self.key().map_err(ExtError::into_engine_error)?;
        let start = (self.clock)();
        match self.provider(gen, text, &voice, rate, key.as_ref())? {
            Ok(pcm) => {
                if self.health.record_success() {
                    self.notify(None, None, "recovered".into());
                }
                Ok(TestResult {
                    voice,
                    ms: (self.clock)().saturating_duration_since(start).as_millis() as u64,
                    pcm,
                })
            }
            Err(e) => Err(e.into_engine_error()),
        }
    }

    fn voice_entry(&self, v: adapter::VoiceInfo) -> Voice {
        Voice {
            id: v.id,
            name: v.name,
            language: v.language,
            engine: self.id,
            license_class: LicenseClass::External,
            installed: true,
        }
    }

    fn with_profile_voice(&self, mut list: Vec<Voice>) -> Vec<Voice> {
        if let Some(v) = self.profile.effective_voice() {
            if !list.iter().any(|x| x.id == v) {
                list.insert(0, self.voice_entry(adapter::VoiceInfo::named(&v)));
            }
        }
        list
    }

    /// Whether `voice` is in the last fetched list (`None`: none fetched).
    fn voice_listed(&self, voice: &str) -> Option<bool> {
        self.voices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|(_, list)| list.iter().any(|v| v.id == voice))
    }

    /// Fetch a voice list, following the pages of a paged one (`None`: muted
    /// meanwhile, so no further page was asked for).
    fn fetch_voices(
        &self,
        adapter: &Arc<dyn Adapter>,
        first: HttpRequest,
        key: Option<&Secret>,
    ) -> std::result::Result<Option<Vec<adapter::VoiceInfo>>, ExtError> {
        let mut out = Vec::new();
        let mut next = Some(first);
        for _ in 0..VOICES_MAX_PAGES {
            let Some(request) = next.take() else { break };
            // Muted meanwhile: no further page (and no partial list).
            if self.held() {
                return Ok(None);
            }
            let r = execute(&self.voices_agent, &request, MAX_LIST_BODY, &self.host)?;
            if !r.ok() {
                return Err(adapter.map_error(&r, "", None));
            }
            out.extend(adapter.parse_voices(&r.body)?);
            next = adapter.next_voices_page(&r.body, key);
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|v| seen.insert(v.id.clone()));
        // A mute during the last page: the list is not kept either.
        if self.held() {
            return Ok(None);
        }
        Ok(Some(out))
    }

    fn voice_source(&self, key: Option<&Secret>) -> VoiceSource {
        match &self.backend {
            Backend::Http(a) => a.voices(key),
            Backend::Command(c) => VoiceSource::Fixed(c.voices()),
        }
    }

    /// Whether the voice cache is older than `VOICES_TTL` (or empty), and
    /// no fetch failed in the last `VOICES_RETRY`.
    pub fn voices_stale(&self) -> bool {
        let now = (self.clock)();
        let failed = *self.voices_failed.lock().unwrap_or_else(|p| p.into_inner());
        if failed.is_some_and(|at| now.saturating_duration_since(at) < VOICES_RETRY) {
            return false;
        }
        self.voices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_none_or(|(at, _)| now.saturating_duration_since(*at) >= VOICES_TTL)
    }
}

impl Engine for External {
    fn id(&self) -> EngineId {
        self.id
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::External
    }

    fn voices(&self) -> Vec<Voice> {
        let cached = self
            .voices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|(_, v)| v.clone());
        let list = match cached {
            Some(v) => v,
            None => match self.voice_source(None) {
                VoiceSource::Fixed(v) => v.into_iter().map(|v| self.voice_entry(v)).collect(),
                VoiceSource::Fetch { .. } => Vec::new(),
            },
        };
        self.with_profile_voice(list)
    }

    fn refresh_voices(&self) -> Result<Vec<Voice>> {
        // Muted: the last list known, nothing fetched.
        if self.held() {
            return Ok(self.voices());
        }
        let key = self.keys.resolve(&self.profile).ok().flatten();
        let list = match (&self.backend, self.voice_source(key.as_ref())) {
            (_, VoiceSource::Fixed(v)) => v,
            (
                Backend::Http(adapter),
                VoiceSource::Fetch {
                    request,
                    empty_on_error,
                },
            ) => match self.fetch_voices(adapter, request, key.as_ref()) {
                Ok(Some(v)) => v,
                Ok(None) => return Ok(self.voices()),
                Err(_) if empty_on_error => Vec::new(),
                Err(e) => {
                    *self.voices_failed.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some((self.clock)());
                    return Err(e.into_engine_error());
                }
            },
            (Backend::Command(_), VoiceSource::Fetch { .. }) => Vec::new(),
        };
        let list: Vec<Voice> = list.into_iter().map(|v| self.voice_entry(v)).collect();
        *self.voices_failed.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.voices.lock().unwrap_or_else(|p| p.into_inner()) = Some(((self.clock)(), list));
        Ok(self.voices())
    }

    fn accepts_unlisted_voices(&self) -> bool {
        true
    }

    fn lookahead(&self) -> usize {
        self.profile.prefetch()
    }

    fn chunk_chars(&self) -> usize {
        self.profile.chunk_chars()
    }

    /// No network call (a cold profile must not send text or spend quota):
    /// only checks that a needed key resolves.
    fn warm(&self) -> Result<()> {
        match self.key() {
            Ok(_) => Ok(()),
            Err(e) => {
                self.health.block(e.reason, e.message.clone());
                if self.fallback.is_some() {
                    Ok(())
                } else {
                    Err(e.into_engine_error())
                }
            }
        }
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        // The marks at `begin`, so a cancel between it and here counts;
        // taken before the hold is checked (`mark`).
        let begun = self.begun.lock().unwrap_or_else(|p| p.into_inner()).take();
        let (epoch, gen) = begun.unwrap_or_else(|| self.mark());
        hold::race_point();
        // Muted: nothing is sent, not even a chunk begun before the mute
        // (a lookahead one), whose cancel generation the hold moved on.
        if self.held() {
            return self.quiet(text, rate);
        }
        if gen != self.cancel.generation() {
            if self.cut_by_hold(epoch) {
                return self.quiet(text, rate);
            }
            return Err(Error::Cancelled);
        }
        let voice = match self.voice_for(voice) {
            Ok(v) => v,
            Err(e) => {
                self.health.block(e.reason, e.message.clone());
                return self.fallback(text, rate, e);
            }
        };
        if let Some(pcm) = self.cache.get(&voice, rate, text) {
            return Ok(Box::new(pcm.into_iter().map(Ok)));
        }
        // A missing key is re-checked at every chunk, without a request.
        if let Some(b) = self.health.blocked() {
            if b.reason == Reason::NoKey && matches!(self.key(), Ok(Some(_))) {
                self.health.clear_key_block();
            }
        }
        if let Some((reason, message)) = self.health.skip(&voice) {
            // A block found without a request (a missing key at `warm`)
            // is logged at the episode's first fallback, like a failure.
            if self.health.cue_pending() {
                self.notify(Some(reason), None, message.clone());
            }
            return self.fallback(text, rate, ExtError::new(reason, message));
        }
        let key = match self.key() {
            Ok(k) => k,
            Err(e) => {
                self.health.block(e.reason, e.message.clone());
                self.notify(Some(e.reason), None, e.message.clone());
                return self.fallback(text, rate, e);
            }
        };
        let answer = match self.provider(gen, text, &voice, rate, key.as_ref()) {
            // The hold was raised while the request ran (even if lifted
            // again since): it was cut, and the chunk is spoken locally.
            Err(Error::Cancelled) if self.cut_by_hold(epoch) => return self.quiet(text, rate),
            other => other?,
        };
        match answer {
            Ok(pcm) => {
                if self.health.record_success() {
                    self.notify(None, None, "recovered".into());
                }
                self.cache.put(&voice, rate, text, pcm.clone());
                Ok(Box::new(pcm.into_iter().map(Ok)))
            }
            Err(e) => {
                self.health.record_failure(&e, &voice);
                self.notify(Some(e.reason), e.status, e.message.clone());
                self.fallback(text, rate, e)
            }
        }
    }

    fn begin(&self) {
        *self.begun.lock().unwrap_or_else(|p| p.into_inner()) = Some(self.mark());
    }

    fn cancel(&self) {
        self.cancel.cancel();
        if let Some(f) = &self.fallback {
            f.cancel();
        }
    }

    fn status(&self) -> EngineStatus {
        let fallback = self.fallback.as_ref().map(|f| f.id());
        let (readiness, reason, message) = match self.health.view() {
            View::Healthy => return EngineStatus::ready(),
            View::Waiting { reason, message } => (Readiness::Waiting, reason, message),
            View::Blocked { reason, message } => (Readiness::Unavailable, reason, message),
        };
        EngineStatus {
            readiness,
            progress: None,
            fallback,
            message: Some(message),
            reason: Some(reason),
        }
    }
}
