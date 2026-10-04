//! The `system` extension (spec 4.4) on top of `sonara_system` (L4): other
//! apps' audio while speech plays (`audio_mode` `off`, `duck` or `pause`,
//! `duck_level`), global hotkeys (`hotkeys`) and the settings page
//! (`settings_url`), with voice previews (`preview`) and the runtime's
//! details (`runtime`) for the page, and the spoken control cues
//! (`cues`: "Paused.", "Muted.", "Rate 250.", ... after a hotkey or a
//! change of `mute_level`, `audio_mode` or `duck_level`; the `cues` event
//! stream reports them).
//!
//! **Enabled** (instance-wide, like the other extensions) once a client
//! asks for it: its keys answer and the settings page is served. **Armed**
//! while it is needed: while a TCP client that asked for it is connected,
//! or for good once a client asked for it with `keep_alive`. Only an armed
//! extension ducks or pauses other apps and holds the hotkeys; when the
//! last client that needed it leaves, other apps are restored at once and
//! the hotkeys are released. The startup sweep and the restore at exit run
//! whatever the state (never leave other apps ducked or paused).
use crate::agent_ext;
use crate::channels_ext;
use crate::config::Store;
use crate::cues::{self, Cues};
use crate::protocol::{bad, opt_str, reader_failure, After, Handled};
use crate::wire::{Code, Failure};
use serde_json::{json, Map, Value};
use sonara_agent::{Earcon, FlushReport, Flushed};
use sonara_reader::{Control, Key, ReaderHandle, Registry, RATE_MAX, RATE_MIN};
use sonara_system::audio::{AudioConfig, AudioControl, AudioMode};
use sonara_system::hotkeys::Hotkeys;
use sonara_system::keymap::{self, Action};
use sonara_system::{LogFn, Platform};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const NAME: &str = "system";

/// `set`/`get` keys of the extension.
pub const KEYS: &[&str] = &[
    "audio_mode",
    "duck_level",
    "hotkeys",
    "settings_url",
    "runtime",
];

/// Message types of the extension.
pub const TYPES: &[&str] = &["preview", "shutdown"];

/// What a voice preview says unless the request gives a text.
pub const PREVIEW_TEXT: &str = "Hello. This is how Sonara sounds with this voice.";
/// Longest preview text, in characters.
pub const PREVIEW_MAX: usize = 300;

/// One rate step of the faster and slower hotkeys (words per minute).
pub const RATE_STEP: u32 = 25;

/// Where the extension lives: the platform, the home and the HTTP port
/// (for `settings_url`).
pub struct SystemHost {
    pub platform: Platform,
    pub home: PathBuf,
    pub http_port: u16,
    pub token: String,
    /// Engines for voice previews and spoken cues: a registry of their own
    /// (built like the reader's), so a preview or a cue never cancels or
    /// waits for the reader's synthesis. `None`: `preview` is
    /// `E_UNSUPPORTED` and cues are reported but not heard. Shared, so the
    /// host can add external engine profiles while it runs.
    pub previews: Option<Arc<Registry>>,
}

/// What a hotkey acts on: the same layers a client drives.
pub struct HotkeyTarget {
    pub reader: ReaderHandle,
    pub channels: channels_ext::Slot,
    pub agent: agent_ext::Slot,
    /// The server's takeover flag: nothing starts once it is set.
    pub retiring: Arc<Mutex<bool>>,
    /// Hotkeys that change a setting (mute cycle, faster, slower) persist
    /// it like a `set`.
    pub store: Arc<Store>,
    /// The spoken cues (set by `SystemExt::new`).
    pub cues: Option<Arc<Cues>>,
    /// The support log: one line per hotkey action, and the audio worker's
    /// lines (other apps paused, resumed, ducked, restored). Else stderr.
    pub log: Option<LogFn>,
}

impl HotkeyTarget {
    /// Carry out one hotkey action like the matching protocol request,
    /// with one log line saying what it did.
    pub fn perform(&self, action: Action) {
        let retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
        if *retiring {
            return;
        }
        let outcome = self.apply(action);
        sonara_system::log::emit(
            self.log.as_ref(),
            &crate::support_log::hotkey_line(action.as_str(), &outcome),
        );
    }

    fn control(&self, c: Control) -> Result<(), String> {
        match self.channels.get() {
            Some(ch) => ch.control(c, None).map_err(|e| e.to_string()),
            None => self.reader.control(c).map_err(|e| e.to_string()),
        }
    }

    fn earcon(&self, e: Earcon) {
        if let Some(a) = self.agent.get() {
            let _ = a.earcon(e);
        }
    }

    fn cue(&self, text: &str, key: Option<&'static str>) {
        if let Some(c) = &self.cues {
            c.speak(text, key);
        }
    }

    fn busy(&self) -> bool {
        let reader = self
            .reader
            .state()
            .map(|s| s.now_playing.is_some() || s.queued > 0)
            .unwrap_or(false);
        reader || self.channels.get().is_some_and(|c| !c.is_idle())
    }

    /// A channel's label, else its id.
    fn session_label(&self, channel: &str) -> String {
        self.channels
            .get()
            .and_then(|ch| ch.channel(channel))
            .and_then(|c| c.label)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| channel.to_string())
    }

    /// A channel's label (else its id) as a log value.
    fn session_value(&self, channel: &str) -> String {
        sonara_system::log::value(&self.session_label(channel))
    }

    /// Carry out `action`; what it did, for the log line, when the action
    /// alone does not say it.
    fn apply(&self, action: Action) -> Result<Option<String>, String> {
        match action {
            Action::Restart => self.control(Control::Restart).map(|_| None),
            Action::Pause => {
                // "Paused." / "Resumed." (Python controls.py): spoken over
                // the paused reader, so the user hears what the key did.
                let before = self.reader.state().ok();
                self.control(Control::Toggle)?;
                let Some(s) = before.filter(|s| s.now_playing.is_some()) else {
                    return Ok(Some("idle".into()));
                };
                self.cue(if s.paused { "Resumed." } else { "Paused." }, None);
                Ok(Some(if s.paused { "resumed" } else { "paused" }.into()))
            }
            Action::Flush => {
                // Flush (#228): stop only the session being read (its
                // item, unread text and summary work); the other sessions
                // are read next as usual. Mute silences everything.
                // Without channels it is `control stop`, as before.
                let (flushed, scope) = match (self.agent.get(), self.channels.get()) {
                    (Some(a), _) => (
                        a.flush().map_err(|e| e.to_string())?,
                        Some(a.settings().flush_scope),
                    ),
                    (None, Some(ch)) => (
                        FlushReport {
                            flushed: ch.stop_reading("flush").map_err(|e| e.to_string())?,
                            others: Vec::new(),
                        },
                        None,
                    ),
                    (None, None) => {
                        let had = self.busy();
                        self.control(Control::Stop)?;
                        let f = if had {
                            Flushed::Direct
                        } else {
                            Flushed::Nothing
                        };
                        (
                            FlushReport {
                                flushed: f,
                                others: Vec::new(),
                            },
                            None,
                        )
                    }
                };
                let had = flushed.flushed != Flushed::Nothing || !flushed.others.is_empty();
                self.earcon(if had { Earcon::Nav } else { Earcon::NavEdge });
                let mut detail = match &flushed.flushed {
                    Flushed::Channel(id) => format!("session={}", self.session_value(id)),
                    Flushed::Announcement(id) => {
                        format!("announcement session={}", self.session_value(id))
                    }
                    Flushed::Direct => "direct".into(),
                    Flushed::Nothing => "idle".into(),
                };
                if let Some(scope) = scope {
                    detail.push_str(&format!(" scope={}", scope.as_str()));
                }
                if !flushed.others.is_empty() {
                    let names: Vec<String> = flushed
                        .others
                        .iter()
                        .map(|id| self.session_label(id))
                        .collect();
                    let names = sonara_system::log::value(&names.join(","));
                    detail.push_str(&format!(" others={names}"));
                }
                Ok(Some(detail))
            }
            Action::Mute => match self.agent.get() {
                // The mute cycle: unmuted, muted (earcons on), super muted.
                Some(a) => {
                    let next = (a.settings().mute_level + 1) % 3;
                    a.set_mute_level(next).map_err(|e| e.to_string())?;
                    self.store.record("mute_level", &json!(next));
                    self.cue(cues::mute_level_cue(u64::from(next)), None);
                    Ok(Some(format!("level={next}")))
                }
                None => {
                    let muted = self.reader.state().map(|s| s.muted).unwrap_or(false);
                    let c = if muted {
                        Control::Unmute
                    } else {
                        Control::Mute
                    };
                    self.reader.control(c).map_err(|e| e.to_string())?;
                    // A muted reader plays clips silently: only the unmute
                    // is heard.
                    self.cue(if muted { "Unmuted." } else { "Muted." }, None);
                    Ok(Some(if muted { "unmuted" } else { "muted" }.into()))
                }
            },
            Action::NextChannel => {
                let Some(ch) = self.channels.get() else {
                    return Ok(Some("session=none".into()));
                };
                let next = ch.next_channel().map_err(|e| e.to_string())?;
                let detail = match &next {
                    Some(t) => format!("session={}", self.session_value(t)),
                    None => "session=none".into(),
                };
                match next {
                    // An announced switch chimes as its "Session changed"
                    // announcement is fed (the agent's L2 hook: earcon
                    // first); a switch that is not announced (announcements
                    // off, a channel without a label) still chimes here.
                    Some(t) => {
                        let announced =
                            ch.announce() && ch.channel(&t).is_some_and(|c| c.label.is_some());
                        if !announced {
                            self.earcon(Earcon::SessionChange);
                        }
                    }
                    // Python controls.py: a spoken "No session." (it had no
                    // earcon for it).
                    None => self.cue("No session.", None),
                }
                Ok(Some(detail))
            }
            Action::Faster | Action::Slower => {
                let rate = match self.reader.get(Key::Rate) {
                    Ok(sonara_reader::Value::Number(n)) => n as u32,
                    _ => return Ok(None),
                };
                let next = if action == Action::Faster {
                    rate.saturating_add(RATE_STEP).min(RATE_MAX)
                } else {
                    rate.saturating_sub(RATE_STEP).max(RATE_MIN)
                };
                self.reader
                    .set(Key::Rate, sonara_reader::Value::Number(u64::from(next)))
                    .map_err(|e| e.to_string())?;
                self.store.record("rate", &json!(next));
                self.cue(&cues::rate_cue(u64::from(next)), Some("rate"));
                Ok(Some(format!("rate={next}")))
            }
        }
    }
}

#[derive(Default)]
struct Holds {
    /// Connected TCP clients that asked for the extension.
    clients: usize,
    /// A client asked for it with `keep_alive`: armed for good.
    sticky: bool,
    armed: bool,
}

pub struct SystemExt {
    audio: Arc<AudioControl>,
    platform: Platform,
    keymap_path: PathBuf,
    settings_url: String,
    http_port: u16,
    token: String,
    target: Arc<HotkeyTarget>,
    enabled: AtomicBool,
    holds: Mutex<Holds>,
    /// Held across an arm or disarm, the flag change and its side effects
    /// together: a release and a new hold never interleave so that the
    /// extension ends up armed with ducking off and no hotkeys.
    transition: Mutex<()>,
    hotkeys: Mutex<Option<Hotkeys>>,
    previews: Option<Arc<Registry>>,
    /// One preview or cue synthesis at a time.
    previewing: Arc<Mutex<()>>,
    cues: Arc<Cues>,
    started: std::time::Instant,
    store: Arc<Store>,
}

impl SystemExt {
    /// The extension on this host; the persisted `audio_mode` and
    /// `duck_level` apply from the start (nothing is ducked or paused
    /// before the extension is armed).
    pub fn new(host: SystemHost, mut target: HotkeyTarget) -> SystemExt {
        let audio = AudioControl::new(
            &host.platform,
            AudioConfig::new(host.home.join("state")).with_log(target.log.clone()),
        );
        let previews = host.previews;
        let previewing = Arc::new(Mutex::new(()));
        let cues = Arc::new(Cues::new(
            target.reader.clone(),
            previews.clone(),
            previewing.clone(),
        ));
        target.cues = Some(cues.clone());
        let store = target.store.clone();
        if let Some(mode) = store
            .value("audio_mode")
            .as_str()
            .and_then(AudioMode::parse)
        {
            audio.set_mode(mode);
        }
        if let Some(level) = store.value("duck_level").as_u64() {
            audio.set_duck_level(level.min(100) as u8);
        }
        SystemExt {
            audio: Arc::new(audio),
            platform: host.platform,
            keymap_path: host.home.join("keymap.json"),
            settings_url: format!(
                "http://127.0.0.1:{}/settings?token={}",
                host.http_port, host.token
            ),
            http_port: host.http_port,
            token: host.token,
            target: Arc::new(target),
            enabled: AtomicBool::new(false),
            holds: Mutex::new(Holds::default()),
            transition: Mutex::new(()),
            hotkeys: Mutex::new(None),
            previews,
            previewing,
            cues,
            started: std::time::Instant::now(),
            store,
        }
    }

    /// The startup sweep: restore what a previous runtime left ducked or
    /// paused. Blocks until done.
    pub fn recover(&self) {
        self.audio.recover();
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn is_armed(&self) -> bool {
        self.lock_holds().armed
    }

    pub fn audio(&self) -> &AudioControl {
        &self.audio
    }

    /// `GET /settings`: the page, or an HTTP status and message.
    pub fn settings_page(
        &self,
        host: Option<&str>,
        query: Option<&str>,
    ) -> Result<String, (u16, &'static str)> {
        if !self.is_enabled() {
            return Err((404, "the settings page needs the system extension"));
        }
        crate::settings_page::render(host, query, self.http_port, &self.token)
    }

    fn lock_holds(&self) -> std::sync::MutexGuard<'_, Holds> {
        self.holds.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_transition(&self) -> std::sync::MutexGuard<'_, ()> {
        self.transition.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Enable the extension (idempotent): from now on it follows the
    /// reader's state.
    pub fn enable(&self, reader: &ReaderHandle) -> Result<(), Failure> {
        if self.enabled.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        match reader.subscribe() {
            Ok(rx) => {
                // Read after subscribing: enabled mid-item engages at once.
                let now = reader.state().ok();
                self.audio.follow(rx, now.as_ref());
                Ok(())
            }
            Err(e) => {
                self.enabled.store(false, Ordering::SeqCst);
                Err(Failure::new(Code::Engine, e.to_string()))
            }
        }
    }

    /// A TCP client that asked for the extension: armed while it lives.
    pub fn hold(self: &Arc<Self>) -> SystemHold {
        let _t = self.lock_transition();
        let arm = {
            let mut h = self.lock_holds();
            h.clients += 1;
            !std::mem::replace(&mut h.armed, true)
        };
        if arm {
            self.arm();
        }
        SystemHold(self.clone())
    }

    /// A client asked with `keep_alive`: armed until the runtime exits.
    pub fn keep(&self) {
        let _t = self.lock_transition();
        let arm = {
            let mut h = self.lock_holds();
            h.sticky = true;
            !std::mem::replace(&mut h.armed, true)
        };
        if arm {
            self.arm();
        }
    }

    fn release(&self) {
        let _t = self.lock_transition();
        let disarm = {
            let mut h = self.lock_holds();
            h.clients = h.clients.saturating_sub(1);
            if h.clients == 0 && !h.sticky && h.armed {
                h.armed = false;
                true
            } else {
                false
            }
        };
        if disarm {
            // Restore other apps at once, then let the hotkeys go.
            self.audio.arm(false);
            self.stop_hotkeys();
        }
    }

    fn arm(&self) {
        self.audio.arm(true);
        self.start_hotkeys();
    }

    fn start_hotkeys(&self) {
        let mut slot = self.hotkeys.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut old) = slot.take() {
            old.stop();
        }
        let (bindings, problems) = keymap::resolve(&keymap::load(&self.keymap_path));
        for p in problems {
            eprintln!("sonarad: keymap: {p}");
        }
        let target = self.target.clone();
        let hk = Hotkeys::start(
            self.platform.registrar.clone(),
            bindings,
            Arc::new(move |a| target.perform(a)),
        );
        for c in hk.collisions() {
            eprintln!(
                "sonarad: hotkey {} not registered (error {}{})",
                c.action.as_str(),
                c.error,
                if c.already_owned() {
                    ": another program owns it"
                } else {
                    ""
                }
            );
        }
        *slot = Some(hk);
    }

    fn stop_hotkeys(&self) {
        let old = self
            .hotkeys
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(mut hk) = old {
            hk.stop();
        }
    }

    /// The keymap changed: register again while armed.
    fn reload_hotkeys(&self) {
        let _t = self.lock_transition();
        if self.is_armed() {
            self.start_hotkeys();
        }
    }

    /// The runtime exits: release the hotkeys and restore other apps.
    pub fn shutdown(&self) {
        self.stop_hotkeys();
        self.cues.shutdown();
        self.audio.shutdown();
    }

    /// Speak a control cue (module docs), once the extension is enabled.
    pub fn cue(&self, text: &str, key: Option<&'static str>) {
        if self.is_enabled() {
            self.cues.speak(text, key);
        }
    }

    /// The spoken cues (the `cues` event stream).
    pub fn cues(&self) -> &Cues {
        &self.cues
    }

    /// `get hotkeys`: the bindings, what is registered, the key names and
    /// the AltGr warnings.
    pub fn hotkeys_json(&self) -> Value {
        let km = keymap::load(&self.keymap_path);
        let (resolved, problems) = keymap::resolve(&km);
        let altgr = keymap::altgr_conflicts(&resolved, self.platform.layout.as_ref());
        let slot = self.hotkeys.lock().unwrap_or_else(|p| p.into_inner());
        let active = slot.is_some();
        let bindings: Vec<Value> = km
            .bindings
            .iter()
            .map(|(action, b)| {
                let r = resolved.iter().find(|r| r.action == *action);
                let collision = slot
                    .as_ref()
                    .and_then(|h| h.collisions().iter().find(|c| c.action == *action));
                let registered = slot
                    .as_ref()
                    .is_some_and(|h| h.registered().iter().any(|r| r.action == *action));
                json!({
                    "action": action.as_str(),
                    "key": b.key.as_deref().filter(|k| !k.is_empty()),
                    "mods": b.mods,
                    "combo": r.map(|r| keymap::combo_label(r.mods, r.vk)),
                    "registered": registered,
                    "error": collision.map(|c| if c.already_owned() {
                        "already_owned".to_string()
                    } else {
                        format!("error {}", c.error)
                    }),
                    "altgr": altgr.iter().find(|a| a.action == *action).map(|a| a.character.clone()),
                })
            })
            .collect();
        json!({
            "active": active,
            "bindings": bindings,
            "keys": keymap::key_names(),
            "mods": keymap::mod_names(),
            "problems": problems,
        })
    }

    /// `set` or `get` of one of `KEYS`.
    pub fn setting(&self, key: &str, value: Option<&Value>) -> Handled {
        if let Some(v) = value {
            match key {
                "audio_mode" => {
                    let mode = v
                        .as_str()
                        .and_then(AudioMode::parse)
                        .ok_or_else(|| bad("'audio_mode' is \"off\", \"duck\" or \"pause\""))?;
                    let changed = self.audio.status().mode != mode;
                    self.audio.set_mode(mode);
                    if let Some(c) = cues::audio_mode_cue(mode.as_str()).filter(|_| changed) {
                        self.cue(c, Some("audio_mode"));
                    }
                }
                "duck_level" => {
                    let n = v
                        .as_u64()
                        .filter(|n| *n <= 100)
                        .ok_or_else(|| bad("'duck_level' is an integer 0 to 100"))?;
                    let changed = u64::from(self.audio.status().duck_level) != n;
                    self.audio.set_duck_level(n as u8);
                    if changed {
                        self.cue(&cues::duck_level_cue(n), Some("duck_level"));
                    }
                }
                "hotkeys" => {
                    self.set_hotkeys(v)?;
                    self.reload_hotkeys();
                }
                _ => return Err(bad(format!("'{key}' is read-only"))),
            }
        }
        let s = self.audio.status();
        let value = match key {
            "audio_mode" => json!(s.mode.as_str()),
            "duck_level" => json!(s.duck_level),
            "hotkeys" => self.hotkeys_json(),
            "runtime" => self.runtime_json(),
            _ => json!(self.settings_url),
        };
        let mut f = Map::new();
        f.insert("key".into(), json!(key));
        f.insert("value".into(), value);
        Ok((f, After::Nothing))
    }

    /// `get runtime`: the process and where its settings live.
    fn runtime_json(&self) -> Value {
        json!({
            "pid": std::process::id(),
            "uptime_s": self.started.elapsed().as_secs(),
            "http_port": self.http_port,
            "config": self.store.config_path().map(|p| p.display().to_string()),
            "previews": self.previews.is_some(),
            // The saved voice, which may differ from the voice in force when
            // this engine lacks it (it applies once it is available).
            "saved_voice": self.store.user("voice").unwrap_or(Value::Null),
        })
    }

    /// `preview {voice?, text?}`: say a short sample with a voice of the
    /// current engine (default: the voice in force) at the current rate,
    /// without touching the queue. It is synthesized on the extension's
    /// own engines and played as a clip mixed over whatever is reading
    /// (`ReaderHandle::play_clip`, like an earcon): nothing is paused,
    /// skipped or queued again, and a muted reader plays it silently.
    pub fn preview(&self, reader: &ReaderHandle, m: &Map<String, Value>) -> Handled {
        let engines = self
            .previews
            .as_ref()
            .ok_or_else(|| Failure::new(Code::Unsupported, "this runtime has no voice previews"))?;
        let text = opt_str(m, "text")?
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(PREVIEW_TEXT);
        let text: String = text.chars().take(PREVIEW_MAX).collect();
        let engine_id = match reader.get(Key::Engine).map_err(reader_failure)? {
            sonara_reader::Value::Text(t) => t,
            _ => return Err(Failure::new(Code::Engine, "no engine")),
        };
        let engine = engines
            .get(&engine_id)
            .map_err(|e| reader_failure(sonara_reader::Error::Engine(e)))?;
        let wanted = match opt_str(m, "voice")? {
            Some(v) if !v.is_empty() => Some(v.to_string()),
            _ => match reader.get(Key::Voice).map_err(reader_failure)? {
                sonara_reader::Value::Text(t) => Some(t),
                _ => None,
            },
        };
        let voice = match &wanted {
            None => String::new(),
            Some(w) => engine
                .voices()
                .into_iter()
                .find(|v| v.id == *w || v.name == *w)
                .map(|v| v.id)
                .or_else(|| engine.accepts_unlisted_voices().then(|| w.clone()))
                .ok_or_else(|| {
                    Failure::new(
                        Code::NotFound,
                        format!("engine '{engine_id}' has no voice '{w}'"),
                    )
                })?,
        };
        let rate = match reader.get(Key::Rate).map_err(reader_failure)? {
            sonara_reader::Value::Number(n) => n as u32,
            _ => 200,
        };
        let engine_err = |e| reader_failure(sonara_reader::Error::Engine(e));
        let chunks = {
            let _one = self.previewing.lock().unwrap_or_else(|p| p.into_inner());
            engine
                .synthesize(&text, &voice, rate)
                .map_err(engine_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(engine_err)?
        };
        let (samples, sample_rate) = cues::mono(&chunks);
        reader
            .play_clip(samples, sample_rate)
            .map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert("engine".into(), json!(engine_id));
        f.insert("voice".into(), json!((!voice.is_empty()).then_some(voice)));
        Ok((f, After::Nothing))
    }

    /// `"reset"`, `{action, key, mods}` (bind) or `{action, key: null}`
    /// (unbind).
    fn set_hotkeys(&self, v: &Value) -> Result<(), Failure> {
        let path = &self.keymap_path;
        if v.as_str() == Some("reset") {
            return keymap::reset(path).map_err(|e| Failure::new(Code::Engine, e));
        }
        let m = v.as_object().ok_or_else(|| {
            bad("'hotkeys' is \"reset\" or {action, key, mods} (key null unbinds)")
        })?;
        let name = m
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("'hotkeys.action' must be a string"))?;
        let action =
            Action::parse(name).ok_or_else(|| bad(format!("unknown hotkey action '{name}'")))?;
        match m.get("key") {
            None | Some(Value::Null) => {
                keymap::unbind(path, action).map_err(|e| Failure::new(Code::Engine, e))
            }
            Some(Value::String(k)) if k.is_empty() => {
                keymap::unbind(path, action).map_err(|e| Failure::new(Code::Engine, e))
            }
            Some(Value::String(k)) => {
                let mods: Vec<String> = match m.get("mods") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(a)) => a
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_string)
                                .ok_or_else(|| bad("'hotkeys.mods' must be a list of strings"))
                        })
                        .collect::<Result<_, _>>()?,
                    Some(_) => return Err(bad("'hotkeys.mods' must be a list of strings")),
                };
                keymap::bind(path, action, k, &mods).map_err(bad)
            }
            Some(_) => Err(bad("'hotkeys.key' must be a string or null")),
        }
    }
}

/// Keeps the extension armed while a client that asked for it is
/// connected; dropping the last one restores other apps.
pub struct SystemHold(Arc<SystemExt>);

impl Drop for SystemHold {
    fn drop(&mut self) {
        self.0.release();
    }
}
