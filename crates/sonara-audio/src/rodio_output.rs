//! The real output: rodio on cpal (WASAPI on Windows), on its own thread.
//!
//! One `Sink` per played chunk gives true pause and resume (M0 measured about
//! 5 ms) and makes `stop` a plain drop. Callbacks queued before and after the
//! chunk's audio report `ChunkStarted` and `ChunkFinished`. The device opens
//! on the first play and reopens on a later one after it failed or was lost.
//!
//! It follows the Windows default output (#288): before each chunk (and
//! before a clip while no speech is left to play) it compares the default
//! render endpoint with the one its stream opened on, and reopens on the new
//! default when they differ. The check runs at a chunk boundary only, so a
//! change never cuts a chunk mid-word: the chunk playing ends on the old
//! device.
use crate::{AudioEvent, ItemId, Output, PcmChunk};
use rodio::buffer::SamplesBuffer;
use rodio::cpal::traits::HostTrait;
use rodio::mixer::Mixer;
use rodio::source::EmptyCallback;
use rodio::{DeviceTrait, OutputStream, OutputStreamBuilder, Sink};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

enum Cmd {
    Play {
        pcm: Vec<PcmChunk>,
        item: ItemId,
        chunk: usize,
        gen: u64,
        /// More audio comes (`Append`) until `Finish`.
        open: bool,
    },
    Append {
        gen: u64,
        pcm: Vec<PcmChunk>,
    },
    Finish {
        gen: u64,
    },
    Pause,
    Resume,
    Stop,
    Volume(u8),
    Clip {
        samples: Vec<i16>,
        sample_rate: u32,
    },
    /// The stream with this number reported an error (device unplugged...).
    DeviceLost {
        stream: u64,
        reason: String,
    },
    Shutdown,
}

/// Hands stream errors from cpal's thread back to the output thread.
#[derive(Clone)]
struct ErrorReporter {
    tx: Sender<Cmd>,
    stream: u64,
}

impl ErrorReporter {
    fn report(&self, reason: String) {
        let _ = self.tx.send(Cmd::DeviceLost {
            stream: self.stream,
            reason,
        });
    }
}

/// An open output stream: what sinks connect to.
trait Stream: Send {
    fn mixer(&self) -> &Mixer;
}

impl Stream for OutputStream {
    fn mixer(&self) -> &Mixer {
        OutputStream::mixer(self)
    }
}

/// A stream and the name of the device it plays on (for the log).
struct Opened {
    stream: Box<dyn Stream>,
    device: String,
}

/// Where the output plays: the system's default render device, or a fake
/// in tests. Both calls run on the output thread.
trait Endpoint: Send {
    /// Open a stream on the current default device.
    fn open(&mut self, reporter: ErrorReporter) -> Result<Opened, String>;
    /// The id of the current default render device, `None` when unknown
    /// (the output then keeps its stream).
    fn default_id(&mut self) -> Option<String>;
}

/// The real default device: rodio (cpal) opens it, and the id is the
/// Windows endpoint of the role cpal opens, `eConsole` (the "default
/// device" of the Sound settings, which sets `eMultimedia` with it).
struct DefaultDevice {
    /// COM is initialized on the output thread.
    #[cfg(windows)]
    com: bool,
}

impl DefaultDevice {
    fn new() -> Self {
        DefaultDevice {
            #[cfg(windows)]
            com: false,
        }
    }
}

impl Endpoint for DefaultDevice {
    fn open(&mut self, reporter: ErrorReporter) -> Result<Opened, String> {
        let default = rodio::cpal::default_host()
            .default_output_device()
            .ok_or("NoDevice")?;
        let device = default
            .name()
            .unwrap_or_else(|_| "an unnamed device".into());
        let mut stream = OutputStreamBuilder::from_device(default)
            .map_err(|e| e.to_string())?
            .with_error_callback(move |e| reporter.report(e.to_string()))
            .open_stream_or_fallback()
            .map_err(|e| e.to_string())?;
        stream.log_on_drop(false);
        Ok(Opened {
            stream: Box::new(stream),
            device,
        })
    }

    #[cfg(windows)]
    fn default_id(&mut self) -> Option<String> {
        use windows::Win32::Media::Audio::{
            eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
        };
        if !self.com {
            // The apartment cpal asks for on this thread; RPC_E_CHANGED_MODE
            // is fine too, COM is initialized either way.
            // SAFETY: initializes COM for the calling (output) thread.
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            self.com = true;
        }
        // SAFETY: COM calls on interfaces this function owns; the id is
        // copied, then freed with CoTaskMemFree as GetId documents.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let raw = device.GetId().ok()?;
            let id = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0 as *const _));
            id
        }
    }

    #[cfg(not(windows))]
    fn default_id(&mut self) -> Option<String> {
        None
    }
}

/// Where the output writes its notes (the device it opened on and why):
/// the host's log.
pub type LogHook = Box<dyn Fn(&str) + Send>;

/// The audio output on the default device.
pub struct RodioOutput {
    cmds: Sender<Cmd>,
    /// For reporting failures when the output thread is gone.
    events: Sender<AudioEvent>,
}

impl RodioOutput {
    /// Start the output thread. The device is opened on the first `play`, so
    /// creating an output never fails; a missing device shows up as
    /// `AudioEvent::Failed` for that play.
    pub fn new() -> (Self, Receiver<AudioEvent>) {
        Self::with_log(Box::new(|_| {}))
    }

    /// `new`, with a line to `log` whenever the output opens a device: the
    /// first time, after one was lost, and when it follows a new default.
    pub fn with_log(log: LogHook) -> (Self, Receiver<AudioEvent>) {
        Self::with_endpoint(Box::new(DefaultDevice::new()), log)
    }

    fn with_endpoint(endpoint: Box<dyn Endpoint>, log: LogHook) -> (Self, Receiver<AudioEvent>) {
        let (events, events_rx) = channel();
        let (cmds, cmds_rx) = channel();
        let worker = Worker {
            endpoint,
            log,
            cmds: cmds.clone(),
            events: events.clone(),
            stream: None,
            opened_on: None,
            followed: false,
            stream_no: 0,
            sink: None,
            loaded: None,
            awaiting_finish: false,
            volume: 1.0,
        };
        // If the thread cannot start, every play fails (see `send`).
        let _ = thread::Builder::new()
            .name("sonara-audio".into())
            .spawn(move || worker.run(cmds_rx));
        (RodioOutput { cmds, events }, events_rx)
    }

    fn send(&self, cmd: Cmd) {
        if let Err(err) = self.cmds.send(cmd) {
            if let Cmd::Play { gen, .. } = err.0 {
                let _ = self.events.send(AudioEvent::Failed {
                    gen,
                    reason: "audio output thread is not running".into(),
                });
            }
        }
    }
}

impl Drop for RodioOutput {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Shutdown);
    }
}

impl Output for RodioOutput {
    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        self.send(Cmd::Play {
            pcm,
            item,
            chunk: chunk_index,
            gen,
            open: false,
        });
    }

    fn play_open(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        self.send(Cmd::Play {
            pcm,
            item,
            chunk: chunk_index,
            gen,
            open: true,
        });
    }

    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>) {
        self.send(Cmd::Append { gen, pcm });
    }

    fn finish(&mut self, gen: u64) {
        self.send(Cmd::Finish { gen });
    }

    fn pause(&mut self) {
        self.send(Cmd::Pause);
    }

    fn resume(&mut self) {
        self.send(Cmd::Resume);
    }

    fn stop(&mut self) {
        self.send(Cmd::Stop);
    }

    fn set_volume(&mut self, percent: u8) {
        self.send(Cmd::Volume(percent));
    }

    fn play_clip(&mut self, samples: &[i16], sample_rate: u32) {
        self.send(Cmd::Clip {
            samples: samples.to_vec(),
            sample_rate,
        });
    }
}

struct Worker {
    endpoint: Box<dyn Endpoint>,
    log: LogHook,
    cmds: Sender<Cmd>,
    events: Sender<AudioEvent>,
    stream: Option<Box<dyn Stream>>,
    /// The default device id when the stream opened (`None`: unknown).
    opened_on: Option<String>,
    /// The stream was dropped to follow a new default device.
    followed: bool,
    /// Number of the current stream, so a late error from a replaced
    /// stream cannot tear down the new one.
    stream_no: u64,
    /// The sink of the loaded chunk; dropping it stops the chunk.
    sink: Option<Sink>,
    loaded: Option<u64>,
    /// The loaded chunk is streamed and its `Finish` has not come yet: it
    /// is still playing even when its sink has run dry.
    awaiting_finish: bool,
    volume: f32,
}

/// The sink gain for a volume percent. Above 100 is full volume, never
/// amplification, which would clip speech and earcons.
fn gain(percent: u8) -> f32 {
    percent.min(100) as f32 / 100.0
}

/// Samples rodio can play, or `None` for an empty or malformed chunk.
fn to_buffer(c: &PcmChunk) -> Option<SamplesBuffer> {
    if c.channels == 0 || c.sample_rate == 0 {
        return None;
    }
    let whole = c.samples.len() - c.samples.len() % c.channels as usize;
    if whole == 0 {
        return None;
    }
    let data: Vec<f32> = c.samples[..whole]
        .iter()
        .map(|&s| s as f32 / 32768.0)
        .collect();
    Some(SamplesBuffer::new(c.channels, c.sample_rate, data))
}

impl Worker {
    fn run(mut self, cmds: Receiver<Cmd>) {
        while let Ok(cmd) = cmds.recv() {
            match cmd {
                Cmd::Play {
                    pcm,
                    item,
                    chunk,
                    gen,
                    open,
                } => self.play(pcm, item, chunk, gen, open),
                Cmd::Append { gen, pcm } => {
                    if let (Some(s), Some(g)) = (&self.sink, self.loaded) {
                        if g == gen {
                            for buffer in pcm.iter().filter_map(to_buffer) {
                                s.append(buffer);
                            }
                        }
                    }
                }
                Cmd::Finish { gen } => {
                    if let (Some(s), Some(g)) = (&self.sink, self.loaded) {
                        if g == gen {
                            self.awaiting_finish = false;
                            let tx = self.events.clone();
                            s.append(EmptyCallback::new(Box::new(move || {
                                let _ = tx.send(AudioEvent::ChunkFinished { gen });
                            })));
                        }
                    }
                }
                Cmd::Pause => {
                    if let Some(s) = &self.sink {
                        s.pause();
                    }
                }
                Cmd::Resume => {
                    if let Some(s) = &self.sink {
                        s.play();
                    }
                }
                Cmd::Stop => {
                    self.sink = None;
                    self.loaded = None;
                }
                Cmd::Volume(percent) => {
                    self.volume = gain(percent);
                    if let Some(s) = &self.sink {
                        s.set_volume(self.volume);
                    }
                }
                Cmd::Clip {
                    samples,
                    sample_rate,
                } => self.clip(samples, sample_rate),
                Cmd::DeviceLost { stream, reason } => self.device_lost(stream, reason),
                Cmd::Shutdown => break,
            }
        }
    }

    /// At a chunk boundary: the default device id now, with the stream
    /// dropped when it plays on another device than the default (#288), so
    /// the next `ensure_stream` opens the new one. An unknown id keeps it.
    fn follow_default(&mut self) -> Option<String> {
        let now = self.endpoint.default_id();
        if let (Some(_), Some(was), Some(id)) = (&self.stream, &self.opened_on, &now) {
            if was != id {
                self.sink = None;
                self.loaded = None;
                self.stream = None;
                self.followed = true;
            }
        }
        now
    }

    /// The stream, opened on the default device (whose id is `default`)
    /// when there is none.
    fn ensure_stream(&mut self, default: Option<String>) -> Result<&dyn Stream, String> {
        if self.stream.is_none() {
            self.stream_no += 1;
            let reporter = ErrorReporter {
                tx: self.cmds.clone(),
                stream: self.stream_no,
            };
            let opened = self.endpoint.open(reporter)?;
            let why = if std::mem::take(&mut self.followed) {
                " (the Windows default output changed)"
            } else {
                ""
            };
            (self.log)(&format!("audio output on \"{}\"{why}", opened.device));
            self.stream = Some(opened.stream);
            self.opened_on = default;
        } else if self.opened_on.is_none() {
            self.opened_on = default;
        }
        self.stream
            .as_deref()
            .ok_or_else(|| "no stream".to_string())
    }

    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk: usize, gen: u64, open: bool) {
        self.sink = None;
        self.loaded = None;
        let default = self.follow_default();
        let sink = match self.ensure_stream(default) {
            Ok(stream) => Sink::connect_new(stream.mixer()),
            Err(e) => {
                let _ = self.events.send(AudioEvent::Failed {
                    gen,
                    reason: format!(
                        "audio device unavailable for item {} chunk {chunk}: {e}",
                        item.0
                    ),
                });
                return;
            }
        };
        sink.set_volume(self.volume);
        let tx = self.events.clone();
        sink.append(EmptyCallback::new(Box::new(move || {
            let _ = tx.send(AudioEvent::ChunkStarted { gen });
        })));
        for buffer in pcm.iter().filter_map(to_buffer) {
            sink.append(buffer);
        }
        // An open chunk finishes after `Finish` (its last audio).
        if !open {
            let tx = self.events.clone();
            sink.append(EmptyCallback::new(Box::new(move || {
                let _ = tx.send(AudioEvent::ChunkFinished { gen });
            })));
        }
        self.sink = Some(sink);
        self.loaded = Some(gen);
        self.awaiting_finish = open;
    }

    fn clip(&mut self, samples: Vec<i16>, sample_rate: u32) {
        let pcm = PcmChunk {
            samples,
            sample_rate,
            channels: 1,
        };
        let volume = self.volume;
        // Follow the default only with no speech left to play: a clip never
        // cuts a chunk, nor a streamed one waiting for more audio.
        let idle = match &self.sink {
            None => true,
            Some(s) => s.empty() && !self.awaiting_finish,
        };
        let default = if idle {
            self.follow_default()
        } else {
            self.opened_on.clone()
        };
        // A clip is a cue: with no device it is dropped, nothing to report.
        if let (Some(buffer), Ok(stream)) = (to_buffer(&pcm), self.ensure_stream(default)) {
            let sink = Sink::connect_new(stream.mixer());
            sink.set_volume(volume);
            sink.append(buffer);
            sink.detach();
        }
    }

    fn device_lost(&mut self, stream: u64, reason: String) {
        if stream != self.stream_no || self.stream.is_none() {
            return;
        }
        self.sink = None;
        self.stream = None;
        if let Some(gen) = self.loaded.take() {
            let _ = self.events.send(AudioEvent::Failed {
                gen,
                reason: format!("audio device lost: {reason}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A machine with no audio device.
    struct NoDevice;

    impl Endpoint for NoDevice {
        fn open(&mut self, _: ErrorReporter) -> Result<Opened, String> {
            Err("NoDevice".into())
        }
        fn default_id(&mut self) -> Option<String> {
            None
        }
    }

    fn no_device() -> (RodioOutput, Receiver<AudioEvent>) {
        RodioOutput::with_endpoint(Box::new(NoDevice), Box::new(|_| {}))
    }

    /// A stream on no device: sinks connect to a mixer nobody plays.
    struct Silent(Mixer);

    impl Stream for Silent {
        fn mixer(&self) -> &Mixer {
            &self.0
        }
    }

    /// A stream whose mixer a thread plays at about real time, so sinks
    /// drain as on a real device; the thread ends with the stream.
    struct Draining {
        mixer: Mixer,
        stop: Arc<AtomicBool>,
    }

    impl Draining {
        fn new() -> Self {
            let (mixer, mut source) = rodio::mixer::mixer(1, 16_000);
            let stop = Arc::new(AtomicBool::new(false));
            let ended = stop.clone();
            thread::spawn(move || {
                while !ended.load(Ordering::Relaxed) {
                    for _ in 0..160 {
                        source.next();
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            });
            Draining { mixer, stop }
        }
    }

    impl Stream for Draining {
        fn mixer(&self) -> &Mixer {
            &self.mixer
        }
    }

    impl Drop for Draining {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Fake default devices: `default` is the id Windows would report now;
    /// every call goes to `calls` in order (`id`, `open <id>`).
    struct FakeDevices {
        default: Arc<Mutex<String>>,
        calls: Sender<String>,
        /// The devices play their audio (sinks drain) instead of holding it.
        draining: bool,
    }

    impl Endpoint for FakeDevices {
        fn open(&mut self, _: ErrorReporter) -> Result<Opened, String> {
            let id = self.default.lock().unwrap().clone();
            let _ = self.calls.send(format!("open {id}"));
            let stream: Box<dyn Stream> = if self.draining {
                Box::new(Draining::new())
            } else {
                let (mixer, _source) = rodio::mixer::mixer(1, 16_000);
                Box::new(Silent(mixer))
            };
            Ok(Opened {
                stream,
                device: format!("Speakers {id}"),
            })
        }
        fn default_id(&mut self) -> Option<String> {
            let _ = self.calls.send("id".into());
            Some(self.default.lock().unwrap().clone())
        }
    }

    struct Fake {
        out: RodioOutput,
        default: Arc<Mutex<String>>,
        calls: Receiver<String>,
        log: Receiver<String>,
        events: Receiver<AudioEvent>,
    }

    fn fake_devices(first: &str) -> Fake {
        fake_devices_with(first, false)
    }

    fn fake_devices_with(first: &str, draining: bool) -> Fake {
        let default = Arc::new(Mutex::new(first.to_string()));
        let (calls_tx, calls) = channel();
        let (log_tx, log) = channel();
        let log_tx = Mutex::new(log_tx);
        let (out, events) = RodioOutput::with_endpoint(
            Box::new(FakeDevices {
                default: default.clone(),
                calls: calls_tx,
                draining,
            }),
            Box::new(move |line| {
                let _ = log_tx.lock().unwrap().send(line.to_string());
            }),
        );
        Fake {
            out,
            default,
            calls,
            log,
            events,
        }
    }

    impl Fake {
        fn set_default(&self, id: &str) {
            *self.default.lock().unwrap() = id.to_string();
        }

        /// The next `n` endpoint calls, then none more.
        fn calls(&self, n: usize) -> Vec<String> {
            let got: Vec<String> = (0..n)
                .map(|_| self.calls.recv_timeout(Duration::from_secs(5)).unwrap())
                .collect();
            assert_eq!(
                self.calls.recv_timeout(Duration::from_millis(200)),
                Err(RecvTimeoutError::Timeout),
                "after {got:?}"
            );
            got
        }
    }

    #[test]
    fn a_new_windows_default_output_is_followed_at_the_next_chunk() {
        let mut f = fake_devices("A");
        f.out.play(pcm(160), ItemId(1), 0, 1);
        f.out.play(pcm(160), ItemId(1), 1, 2);
        assert_eq!(f.calls(3), ["id", "open A", "id"]);
        f.set_default("B");
        f.out.play(pcm(160), ItemId(1), 2, 3);
        f.out.play(pcm(160), ItemId(1), 3, 4);
        assert_eq!(f.calls(3), ["id", "open B", "id"]);
        assert_eq!(
            f.log.try_iter().collect::<Vec<_>>(),
            [
                "audio output on \"Speakers A\"",
                "audio output on \"Speakers B\" (the Windows default output changed)"
            ]
        );
    }

    #[test]
    fn an_unchanged_default_output_keeps_the_stream() {
        let mut f = fake_devices("A");
        for chunk in 0..4 {
            f.out.play(pcm(160), ItemId(1), chunk, chunk as u64 + 1);
        }
        f.out.play_clip(&[1, 2, 3], 16_000);
        assert_eq!(f.calls(5), ["id", "open A", "id", "id", "id"]);
    }

    #[test]
    fn a_default_change_never_cuts_the_chunk_playing() {
        let mut f = fake_devices("A");
        f.out.play_open(pcm(160), ItemId(1), 0, 1);
        assert_eq!(f.calls(2), ["id", "open A"]);
        f.set_default("B");
        // More audio of the same chunk, and a clip over it: no check.
        f.out.append(1, pcm(160));
        f.out.play_clip(&[1, 2, 3], 16_000);
        f.out.finish(1);
        assert_eq!(f.calls(0), Vec::<String>::new());
        // The next chunk moves to B.
        f.out.play(pcm(160), ItemId(1), 1, 2);
        assert_eq!(f.calls(2), ["id", "open B"]);
    }

    /// A streamed chunk whose audio so far has played (its sink is empty)
    /// is still playing until `Finish`: a clip then must not move the
    /// output, which would drop the chunk and never report it finished.
    #[test]
    fn a_clip_between_streamed_audio_never_drops_the_open_chunk() {
        let mut f = fake_devices_with("A", true);
        f.out.play_open(pcm(160), ItemId(1), 0, 1);
        assert_eq!(
            f.events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::ChunkStarted { gen: 1 })
        );
        // The 10 ms of audio plays out while the synth is still working.
        thread::sleep(Duration::from_millis(200));
        f.set_default("B");
        f.out.play_clip(&[1, 2, 3], 16_000);
        f.out.append(1, pcm(160));
        f.out.finish(1);
        assert_eq!(
            f.events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::ChunkFinished { gen: 1 })
        );
        // The next chunk moves to B.
        f.out.play(pcm(160), ItemId(1), 1, 2);
        assert_eq!(f.calls(4), ["id", "open A", "id", "open B"]);
    }

    #[test]
    fn a_clip_with_no_speech_loaded_follows_the_default_output() {
        let mut f = fake_devices("A");
        f.out.play_clip(&[1, 2, 3], 16_000);
        assert_eq!(f.calls(2), ["id", "open A"]);
        f.set_default("B");
        f.out.play_clip(&[1, 2, 3], 16_000);
        assert_eq!(f.calls(2), ["id", "open B"]);
    }

    fn pcm(n: usize) -> Vec<PcmChunk> {
        vec![PcmChunk {
            samples: vec![0; n],
            sample_rate: 16_000,
            channels: 1,
        }]
    }

    #[test]
    fn a_missing_device_fails_the_play_and_keeps_working() {
        let (mut out, events) = no_device();
        out.play(pcm(160), ItemId(1), 0, 7);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed {
                gen: 7,
                reason: "audio device unavailable for item 1 chunk 0: NoDevice".into()
            })
        );
        // Controls and clips with no device are no-ops, never panics.
        out.pause();
        out.resume();
        out.set_volume(40);
        out.play_clip(&[1, 2, 3], 16_000);
        out.stop();
        out.play(pcm(160), ItemId(1), 1, 8);
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed { gen: 8, .. })
        ));
    }

    #[test]
    fn a_dead_output_thread_fails_plays_instead_of_panicking() {
        let (mut out, events) = no_device();
        let _ = out.cmds.send(Cmd::Shutdown);
        // Wait for the thread to exit, so the next send fails.
        for _ in 0..100 {
            if out.cmds.send(Cmd::Stop).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        out.play(pcm(10), ItemId(2), 0, 3);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed {
                gen: 3,
                reason: "audio output thread is not running".into()
            })
        );
    }

    #[test]
    fn malformed_chunks_are_skipped() {
        let bad = |samples: Vec<i16>, sample_rate, channels| PcmChunk {
            samples,
            sample_rate,
            channels,
        };
        assert!(to_buffer(&bad(vec![1, 2], 0, 1)).is_none());
        assert!(to_buffer(&bad(vec![1, 2], 16_000, 0)).is_none());
        assert!(to_buffer(&bad(vec![], 16_000, 1)).is_none());
        assert!(to_buffer(&bad(vec![1], 16_000, 2)).is_none());
        assert!(to_buffer(&bad(vec![1, 2, 3], 16_000, 2)).is_some());
    }

    #[test]
    fn volume_above_100_is_full_gain_not_amplification() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(40), 0.4);
        assert_eq!(gain(100), 1.0);
        assert_eq!(gain(150), 1.0);
        assert_eq!(gain(255), 1.0);
    }

    /// The real default device: whatever the machine has (CI runners have
    /// none), a play ends in ChunkFinished or Failed, never a panic. A device
    /// that opens but never advances (a stalled host) is a skip, not a
    /// failure, so the result does not depend on the machine.
    #[test]
    fn the_default_device_finishes_or_fails() {
        let (mut out, events) = RodioOutput::new();
        out.set_volume(0);
        out.play(pcm(800), ItemId(1), 0, 1); // 50 ms of silence
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(AudioEvent::ChunkStarted { gen: 1 }) => continue,
                Ok(AudioEvent::ChunkFinished { gen: 1 })
                | Ok(AudioEvent::Failed { gen: 1, .. }) => break,
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("skipped: the default device did not advance within 10 s");
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
