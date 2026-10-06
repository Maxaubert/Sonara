//! Engines for facade tests: one that holds every synthesis until the test
//! opens it (or cancels it), one that always fails, and the fake engine
//! under another id.
use sonara_engine::fake::FakeEngine;
use sonara_engine::{
    Engine, EngineId, Error, InputLimit, LicenseClass, PcmChunk, PcmStream, Result, SendMode, Voice,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Synthesizes like the fake engine, but only once `open` was called; a
/// `cancel` ends every synthesis waiting at that moment with `Cancelled`.
#[derive(Default)]
pub struct GateEngine {
    /// (open, cancel epoch)
    state: Mutex<(bool, u64)>,
    changed: Condvar,
    started: AtomicUsize,
    cancels: AtomicUsize,
}

impl GateEngine {
    pub fn open(&self) {
        self.state.lock().unwrap().0 = true;
        self.changed.notify_all();
    }

    pub fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    pub fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }

    /// Wait until `n` syntheses have started.
    pub fn wait_started(&self, n: usize) {
        let end = Instant::now() + super::TIMEOUT;
        while self.started() < n {
            assert!(Instant::now() < end, "only {} started", self.started());
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Engine for GateEngine {
    fn id(&self) -> EngineId {
        EngineId("gate")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut st = self.state.lock().unwrap();
        let epoch = st.1;
        while !st.0 && st.1 == epoch {
            st = self.changed.wait(st).unwrap();
        }
        if st.1 != epoch {
            return Err(Error::Cancelled);
        }
        drop(st);
        let samples = FakeEngine::render(text, voice, rate)?;
        Ok(Box::new(std::iter::once(Ok(PcmChunk {
            samples,
            sample_rate: 16_000,
            channels: 1,
        }))))
    }

    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        self.state.lock().unwrap().1 += 1;
        self.changed.notify_all();
    }
}

/// Lets one synthesis through per permit (`let_through`) and ignores
/// `cancel` (it only counts it): a slow engine whose audio still arrives
/// after its item ended (#269). A synthesis no permit reached within
/// `TIMEOUT` gives up, so a failing test still shuts down.
pub struct TurnstileEngine {
    id: &'static str,
    permits: Mutex<usize>,
    changed: Condvar,
    started: AtomicUsize,
    cancels: AtomicUsize,
    warms: AtomicUsize,
}

impl TurnstileEngine {
    pub fn new(id: &'static str) -> Self {
        TurnstileEngine {
            id,
            permits: Mutex::new(0),
            changed: Condvar::new(),
            started: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            warms: AtomicUsize::new(0),
        }
    }

    pub fn let_through(&self, n: usize) {
        *self.permits.lock().unwrap() += n;
        self.changed.notify_all();
    }

    pub fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }

    /// Wait until `n` syntheses have started. The synthesis thread hands
    /// back a chunk before it starts its next task, so once `n` started,
    /// the results of the first `n - 1` are in the reader's inbox.
    pub fn wait_started(&self, n: usize) {
        wait_count(&self.started, n, "started");
    }

    /// Wait until `n` warm-ups ran (the same ordering as `wait_started`).
    pub fn wait_warmed(&self, n: usize) {
        wait_count(&self.warms, n, "warmed");
    }
}

fn wait_count(count: &AtomicUsize, n: usize, what: &str) {
    let end = Instant::now() + super::TIMEOUT;
    while count.load(Ordering::SeqCst) < n {
        assert!(
            Instant::now() < end,
            "only {} {what}",
            count.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

impl Engine for TurnstileEngine {
    fn id(&self) -> EngineId {
        EngineId(self.id)
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        self.warms.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let end = Instant::now() + super::TIMEOUT;
        let mut permits = self.permits.lock().unwrap();
        while *permits == 0 {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Cancelled);
            }
            permits = self.changed.wait_timeout(permits, left).unwrap().0;
        }
        *permits -= 1;
        drop(permits);
        let samples = FakeEngine::render(text, voice, rate)?;
        Ok(Box::new(std::iter::once(Ok(PcmChunk {
            samples,
            sample_rate: 16_000,
            channels: 1,
        }))))
    }

    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }
}

/// Speaks like the fake engine, but `warm` blocks until `cancel`.
#[derive(Default)]
pub struct SlowWarmEngine {
    inner: FakeEngine,
    /// (warming, cancelled)
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl SlowWarmEngine {
    /// Wait until `warm` is running.
    pub fn wait_warming(&self) {
        let end = Instant::now() + super::TIMEOUT;
        while !self.state.lock().unwrap().0 {
            assert!(Instant::now() < end, "warm never started");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Engine for SlowWarmEngine {
    fn id(&self) -> EngineId {
        EngineId("slow-warm")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.0 = true;
        while !st.1 {
            st = self.changed.wait(st).unwrap();
        }
        Err(Error::Cancelled)
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.inner.synthesize(text, voice, rate)
    }

    fn cancel(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}

/// Never ready, never speaks.
pub struct BrokenEngine;

impl Engine for BrokenEngine {
    fn id(&self) -> EngineId {
        EngineId("broken")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Os
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Err(Error::NoVoices)
    }

    fn synthesize(&self, _: &str, _: &str, _: u32) -> Result<PcmStream> {
        Err(Error::NoVoices)
    }

    fn cancel(&self) {}
}

/// The fake engine as `other`, offering only its `tone` voice.
#[derive(Default)]
pub struct OtherEngine {
    pub inner: FakeEngine,
}

impl Engine for OtherEngine {
    fn id(&self) -> EngineId {
        EngineId("other")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        let mut v = self.inner.voices();
        v.retain(|v| v.id == "tone");
        for voice in &mut v {
            voice.engine = self.id();
        }
        v
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.inner.synthesize(text, voice, rate)
    }

    fn cancel(&self) {
        self.inner.cancel()
    }
}

/// The fake engine with a status the test sets (a model downloading, then
/// ready), as Kokoro reports it.
pub struct StatusEngine {
    pub inner: FakeEngine,
    pub status: Mutex<sonara_engine::EngineStatus>,
}

impl StatusEngine {
    pub fn new(status: sonara_engine::EngineStatus) -> Self {
        StatusEngine {
            inner: FakeEngine::new(),
            status: Mutex::new(status),
        }
    }

    pub fn set(&self, status: sonara_engine::EngineStatus) {
        *self.status.lock().unwrap() = status;
    }
}

impl Engine for StatusEngine {
    fn id(&self) -> EngineId {
        EngineId("status")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.inner.synthesize(text, voice, rate)
    }

    fn cancel(&self) {}

    fn status(&self) -> sonara_engine::EngineStatus {
        self.status.lock().unwrap().clone()
    }
}

/// Like an external engine: any voice id is accepted (it speaks the fake
/// tone and records the voice), and it asks for a deeper prefetch.
pub struct OpenEngine {
    pub id: &'static str,
    pub inner: FakeEngine,
    pub lookahead: usize,
    /// `Engine::send_mode` (#235).
    pub send_mode: SendMode,
    /// `Engine::input_limit` in characters.
    pub limit: usize,
    pub voices_seen: Mutex<Vec<String>>,
}

impl OpenEngine {
    pub fn new(id: &'static str, lookahead: usize) -> Self {
        OpenEngine {
            id,
            inner: FakeEngine::new(),
            lookahead,
            send_mode: SendMode::Sentence,
            limit: 4096,
            voices_seen: Mutex::new(Vec::new()),
        }
    }
}

impl Engine for OpenEngine {
    fn id(&self) -> EngineId {
        EngineId(self.id)
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.voices_seen.lock().unwrap().push(voice.to_string());
        self.inner.synthesize(text, "", rate)
    }

    fn cancel(&self) {
        self.inner.cancel()
    }

    fn lookahead(&self) -> usize {
        self.lookahead
    }

    fn send_mode(&self) -> SendMode {
        self.send_mode
    }

    fn input_limit(&self) -> InputLimit {
        InputLimit::Chars(self.limit)
    }

    fn accepts_unlisted_voices(&self) -> bool {
        true
    }
}

type PieceRx = std::sync::Arc<Mutex<std::sync::mpsc::Receiver<Option<usize>>>>;

/// A streaming engine (#235): each synthesis yields the pieces the test
/// sends (`Some(samples)`) as they come, and ends at `None`.
pub struct StreamEngine {
    rx: PieceRx,
}

impl StreamEngine {
    pub fn new() -> (StreamEngine, std::sync::mpsc::Sender<Option<usize>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            StreamEngine {
                rx: std::sync::Arc::new(Mutex::new(rx)),
            },
            tx,
        )
    }
}

/// The pieces of one synthesis, read when the reader asks for the next.
struct Pieces(PieceRx);

impl Iterator for Pieces {
    type Item = Result<PcmChunk>;

    fn next(&mut self) -> Option<Result<PcmChunk>> {
        let n = self.0.lock().unwrap().recv().ok()??;
        Some(Ok(PcmChunk {
            samples: vec![1; n],
            sample_rate: 16_000,
            channels: 1,
        }))
    }
}

impl Engine for StreamEngine {
    fn id(&self) -> EngineId {
        EngineId("stream")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, _: &str, _: &str, _: u32) -> Result<PcmStream> {
        Ok(Box::new(Pieces(self.rx.clone())))
    }

    fn cancel(&self) {}

    fn streams(&self) -> bool {
        true
    }
}

/// Its warm-up fails (as OneCore's with no usable voices, #274) until the
/// test lets it speak; then it is the fake engine.
#[derive(Default)]
pub struct ColdEngine {
    pub inner: FakeEngine,
    pub speaks: std::sync::atomic::AtomicBool,
}

impl Engine for ColdEngine {
    fn id(&self) -> EngineId {
        EngineId("cold")
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Os
    }

    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }

    fn warm(&self) -> Result<()> {
        Err(Error::NoVoices)
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        if self.speaks.load(std::sync::atomic::Ordering::SeqCst) {
            self.inner.synthesize(text, voice, rate)
        } else {
            Err(Error::NoVoices)
        }
    }

    fn cancel(&self) {}
}
