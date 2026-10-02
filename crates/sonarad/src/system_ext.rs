//! The `system` extension (spec 4.4) on top of `sonara_system` (L4): other
//! apps' audio while speech plays (`audio_mode` `off`, `duck` or `pause`,
//! `duck_level`), global hotkeys (`hotkeys`) and the settings page
//! (`settings_url`).
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
use crate::protocol::{bad, After, Handled};
use crate::wire::{Code, Failure};
use serde_json::{json, Map, Value};
use sonara_agent::Earcon;
use sonara_reader::{Control, Key, ReaderHandle, RATE_MAX, RATE_MIN};
use sonara_system::audio::{AudioConfig, AudioControl, AudioMode};
use sonara_system::hotkeys::Hotkeys;
use sonara_system::keymap::{self, Action};
use sonara_system::Platform;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const NAME: &str = "system";

/// `set`/`get` keys of the extension.
pub const KEYS: &[&str] = &["audio_mode", "duck_level", "hotkeys", "settings_url"];

/// One rate step of the faster and slower hotkeys (words per minute).
pub const RATE_STEP: u32 = 25;

/// Where the extension lives: the platform, the home and the HTTP port
/// (for `settings_url`).
pub struct SystemHost {
    pub platform: Platform,
    pub home: PathBuf,
    pub http_port: u16,
    pub token: String,
}

/// What a hotkey acts on: the same layers a client drives.
pub struct HotkeyTarget {
    pub reader: ReaderHandle,
    pub channels: channels_ext::Slot,
    pub agent: agent_ext::Slot,
    /// The server's takeover flag: nothing starts once it is set.
    pub retiring: Arc<Mutex<bool>>,
}

impl HotkeyTarget {
    /// Carry out one hotkey action like the matching protocol request.
    pub fn perform(&self, action: Action) {
        let retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
        if *retiring {
            return;
        }
        if let Err(e) = self.apply(action) {
            eprintln!("sonarad: hotkey {}: {e}", action.as_str());
        }
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

    fn busy(&self) -> bool {
        let reader = self
            .reader
            .state()
            .map(|s| s.now_playing.is_some() || s.queued > 0)
            .unwrap_or(false);
        reader || self.channels.get().is_some_and(|c| !c.is_idle())
    }

    fn apply(&self, action: Action) -> Result<(), String> {
        match action {
            Action::Restart => self.control(Control::Restart),
            Action::Pause => self.control(Control::Toggle),
            Action::Flush => {
                // Flush to end (#107): silence everything queued or in
                // flight, as `control stop` without a channel.
                let had = self.busy();
                match self.agent.get() {
                    Some(a) => a.stop().map_err(|e| e.to_string())?,
                    None => self.control(Control::Stop)?,
                }
                self.earcon(if had { Earcon::Nav } else { Earcon::NavEdge });
                Ok(())
            }
            Action::Mute => match self.agent.get() {
                // The mute cycle: unmuted, muted (earcons on), super muted.
                Some(a) => {
                    let next = (a.settings().mute_level + 1) % 3;
                    a.set_mute_level(next).map_err(|e| e.to_string())
                }
                None => {
                    let muted = self.reader.state().map(|s| s.muted).unwrap_or(false);
                    let c = if muted {
                        Control::Unmute
                    } else {
                        Control::Mute
                    };
                    self.reader.control(c).map_err(|e| e.to_string())
                }
            },
            Action::NextChannel => {
                let Some(ch) = self.channels.get() else {
                    return Ok(());
                };
                match ch.next_channel().map_err(|e| e.to_string())? {
                    Some(_) => self.earcon(Earcon::SessionChange),
                    None => self.earcon(Earcon::NavEdge),
                }
                Ok(())
            }
            Action::Faster | Action::Slower => {
                let rate = match self.reader.get(Key::Rate) {
                    Ok(sonara_reader::Value::Number(n)) => n as u32,
                    _ => return Ok(()),
                };
                let next = if action == Action::Faster {
                    rate.saturating_add(RATE_STEP).min(RATE_MAX)
                } else {
                    rate.saturating_sub(RATE_STEP).max(RATE_MIN)
                };
                self.reader
                    .set(Key::Rate, sonara_reader::Value::Number(u64::from(next)))
                    .map_err(|e| e.to_string())
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
    hotkeys: Mutex<Option<Hotkeys>>,
}

impl SystemExt {
    pub fn new(host: SystemHost, target: HotkeyTarget) -> SystemExt {
        let audio = AudioControl::new(&host.platform, AudioConfig::new(host.home.join("state")));
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
            hotkeys: Mutex::new(None),
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

    /// Enable the extension (idempotent): from now on it follows the
    /// reader's state.
    pub fn enable(&self, reader: &ReaderHandle) -> Result<(), Failure> {
        if self.enabled.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        match reader.subscribe() {
            Ok(rx) => {
                self.audio.follow(rx);
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
        if self.is_armed() {
            self.start_hotkeys();
        }
    }

    /// The runtime exits: release the hotkeys and restore other apps.
    pub fn shutdown(&self) {
        self.stop_hotkeys();
        self.audio.shutdown();
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
                    self.audio.set_mode(mode);
                }
                "duck_level" => {
                    let n = v
                        .as_u64()
                        .filter(|n| *n <= 100)
                        .ok_or_else(|| bad("'duck_level' is an integer 0 to 100"))?;
                    self.audio.set_duck_level(n as u8);
                }
                "hotkeys" => {
                    self.set_hotkeys(v)?;
                    self.reload_hotkeys();
                }
                _ => return Err(bad("'settings_url' is read-only")),
            }
        }
        let s = self.audio.status();
        let value = match key {
            "audio_mode" => json!(s.mode.as_str()),
            "duck_level" => json!(s.duck_level),
            "hotkeys" => self.hotkeys_json(),
            _ => json!(self.settings_url),
        };
        let mut f = Map::new();
        f.insert("key".into(), json!(key));
        f.insert("value".into(), value);
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
