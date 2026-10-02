//! The Windows platform: Core Audio session volume across every active
//! render device, GSMTC media sessions, RegisterHotKey with a thread message
//! loop, and the keyboard layout for the AltGr check.
//!
//! Live checks against this PC's real audio, media and hotkeys are
//! `#[ignore]` tests in `tests/win_live.rs` (opt-in, they touch the user's
//! session). The unit and conformance tests use `crate::fake`.
use crate::platform::{
    AudioSession, AudioSessions, KeyboardLayout, MediaSession, MediaSessions, PResult, Platform,
    Registrar,
};
use std::sync::Arc;
use windows::core::{Interface, PWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as GsmtcSession,
    GlobalSystemMediaTransportControlsSessionManager as GsmtcManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as GsmtcStatus,
};
use windows::Win32::Foundation::{CloseHandle, LPARAM, WPARAM};
use windows::Win32::Media::Audio::{
    eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, MapVirtualKeyExW, RegisterHotKey, ToUnicodeEx, UnregisterHotKey,
    HOT_KEY_MODIFIERS, MAPVK_VK_TO_VSC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT,
};

/// The real platform.
pub fn platform() -> Platform {
    Platform {
        name: "windows",
        audio: Arc::new(|| Box::new(CoreAudio) as Box<dyn AudioSessions>),
        media: Arc::new(|| Box::new(Gsmtc) as Box<dyn MediaSessions>),
        registrar: Arc::new(|| Box::new(HotkeyThread::new()) as Box<dyn Registrar>),
        layout: Arc::new(Layout),
    }
}

fn msg(e: windows::core::Error) -> String {
    format!("{e} ({:#010x})", e.code().0)
}

/// COM on the calling thread (multithreaded; a repeat call is harmless).
fn com() {
    // SAFETY: plain COM initialisation of the calling thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// The image file name of a process (`vlc.exe`), empty when unknown.
fn process_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    // SAFETY: the handle is closed below; the buffer outlives the call.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

struct CoreAudio;

struct CoreSession {
    pid: u32,
    name: String,
    volume: ISimpleAudioVolume,
}

impl AudioSession for CoreSession {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn volume(&self) -> PResult<f32> {
        // SAFETY: a COM call on a live interface.
        unsafe { self.volume.GetMasterVolume() }.map_err(msg)
    }

    fn set_volume(&self, level: f32) -> PResult<()> {
        // SAFETY: a COM call on a live interface; no event context.
        unsafe {
            self.volume
                .SetMasterVolume(level.clamp(0.0, 1.0), std::ptr::null())
        }
        .map_err(msg)
    }
}

impl AudioSessions for CoreAudio {
    /// Sessions of every active render device, not only the default one:
    /// with a virtual mixer (SteelSeries Sonar, VoiceMeeter) the app playing
    /// media is often on another device. A device or session that cannot
    /// be opened is skipped.
    fn sessions(&self) -> PResult<Vec<Box<dyn AudioSession>>> {
        com();
        // SAFETY: COM calls on interfaces this function owns.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(msg)?;
            let devices = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(msg)?;
            let mut out: Vec<Box<dyn AudioSession>> = Vec::new();
            for i in 0..devices.GetCount().map_err(msg)? {
                let Ok(device) = devices.Item(i) else {
                    continue;
                };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                    continue;
                };
                let Ok(list) = manager.GetSessionEnumerator() else {
                    continue;
                };
                for j in 0..list.GetCount().unwrap_or(0) {
                    let Ok(control) = list.GetSession(j) else {
                        continue;
                    };
                    let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    let Ok(volume) = control.cast::<ISimpleAudioVolume>() else {
                        continue;
                    };
                    let pid = control2.GetProcessId().unwrap_or(0);
                    out.push(Box::new(CoreSession {
                        pid,
                        name: process_name(pid),
                        volume,
                    }));
                }
            }
            Ok(out)
        }
    }
}

struct Gsmtc;

struct GsmtcEntry(GsmtcSession);

impl MediaSession for GsmtcEntry {
    fn app_id(&self) -> PResult<String> {
        self.0
            .SourceAppUserModelId()
            .map(|s| s.to_string())
            .map_err(msg)
    }

    fn is_playing(&self) -> PResult<bool> {
        let info = self.0.GetPlaybackInfo().map_err(msg)?;
        Ok(info.PlaybackStatus().map_err(msg)? == GsmtcStatus::Playing)
    }

    fn pause(&self) -> PResult<()> {
        let done = self.0.TryPauseAsync().map_err(msg)?.join().map_err(msg)?;
        if done {
            Ok(())
        } else {
            Err("the app refused to pause".into())
        }
    }

    fn play(&self) -> PResult<()> {
        let done = self.0.TryPlayAsync().map_err(msg)?.join().map_err(msg)?;
        if done {
            Ok(())
        } else {
            Err("the app refused to play".into())
        }
    }
}

impl MediaSessions for Gsmtc {
    fn sessions(&self) -> PResult<Vec<Box<dyn MediaSession>>> {
        com();
        let manager = GsmtcManager::RequestAsync()
            .map_err(msg)?
            .join()
            .map_err(msg)?;
        let list = manager.GetSessions().map_err(msg)?;
        let mut out: Vec<Box<dyn MediaSession>> = Vec::new();
        for i in 0..list.Size().map_err(msg)? {
            if let Ok(s) = list.GetAt(i) {
                out.push(Box::new(GsmtcEntry(s)));
            }
        }
        Ok(out)
    }
}

/// RegisterHotKey on the creating thread, with that thread's message queue.
struct HotkeyThread {
    thread: u32,
}

impl HotkeyThread {
    fn new() -> Self {
        let mut m = MSG::default();
        // SAFETY: PeekMessage creates this thread's message queue, so a
        // stop posted before the first GetMessage is not lost.
        unsafe {
            let _ = PeekMessageW(&mut m, None, 0, 0, PM_NOREMOVE);
        }
        HotkeyThread {
            // SAFETY: no preconditions.
            thread: unsafe { GetCurrentThreadId() },
        }
    }
}

impl Registrar for HotkeyThread {
    fn register(&mut self, id: i32, modifiers: u32, vk: u32) -> Result<(), u32> {
        // SAFETY: a thread hotkey (no window) on this thread.
        unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(modifiers), vk) }
            .map_err(|e| (e.code().0 as u32) & 0xFFFF)
    }

    fn unregister(&mut self, id: i32) {
        // SAFETY: as register.
        let _ = unsafe { UnregisterHotKey(None, id) };
    }

    fn wait(&mut self) -> Option<i32> {
        loop {
            let mut m = MSG::default();
            // SAFETY: a message loop of this thread.
            let r = unsafe { GetMessageW(&mut m, None, 0, 0) };
            if r.0 == 0 || r.0 == -1 {
                return None;
            }
            if m.message == WM_HOTKEY {
                return Some(m.wParam.0 as i32);
            }
        }
    }

    fn stopper(&self) -> Box<dyn Fn() + Send> {
        let thread = self.thread;
        Box::new(move || {
            // SAFETY: posting WM_QUIT to the pump thread's queue.
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        })
    }
}

struct Layout;

impl KeyboardLayout for Layout {
    /// What AltGr (+ Shift) + `vk` types on this thread's keyboard layout.
    /// The calling thread's layout, not the foreground app's: with per-app
    /// layouts they can differ (as in the Python check, #160).
    fn altgr_char(&self, vk: u32, shift: bool) -> Option<String> {
        // SAFETY: plain keyboard queries with local buffers.
        unsafe {
            let hkl = GetKeyboardLayout(0);
            let mut state = [0u8; 256];
            for k in [0x11usize, 0x12, 0xA2, 0xA5] {
                state[k] = 0x80; // CONTROL, MENU, LCONTROL, RMENU
            }
            if shift {
                state[0x10] = 0x80;
                state[0xA0] = 0x80;
            }
            let scan = MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, Some(hkl));
            let mut buf = [0u16; 8];
            // Flag 0x4: leave the keyboard's dead-key state untouched.
            let n = ToUnicodeEx(vk, scan, &state, &mut buf, 0x4, Some(hkl));
            if n <= 0 {
                return None;
            }
            let ch = String::from_utf16_lossy(&buf[..n as usize]);
            let printable = !ch.trim().is_empty() && !ch.chars().any(char::is_control);
            printable.then_some(ch)
        }
    }
}
