//! The macOS platform: Carbon global hotkeys (`RegisterEventHotKey`, no
//! permission prompt), tap and hold from the hotkey's key-release event,
//! media pause through AppleScript (Music and Spotify) and through Now
//! Playing (`mac_nowplaying`: a browser video, Podcasts and other apps), and the session
//! end from SIGTERM and `NSWorkspaceWillPowerOffNotification`. Ducking is
//! unavailable (`Platform::duck` is false): macOS has no per-app volume
//! without an audio tap, so `duck` mode pauses media instead.
//!
//! **The event pump (plan step 9a).** Carbon delivers hotkey events to the
//! main event queue, and only a thread that dispatches that queue runs the
//! handler: `CFRunLoopRun` alone and `RunCurrentEventLoop` leave the events
//! in the queue (a prototype posted a `kEventHotKeyPressed` that never
//! reached the handler under either). So the main thread runs [`Pump`]
//! (`ReceiveNextEvent` plus `SendEventToEventTarget`, which also runs the
//! main run loop and the main dispatch queue), and [`run_main`] moves the
//! rest of the program to a worker thread. Every Carbon and Text Input
//! Sources call runs on the main thread (`dispatch_sync` to the main
//! queue); the registrar's `wait` is a channel the handler feeds, and its
//! stopper ends that wait. The pump ends with `QuitEventLoop` (the Carbon
//! form of `CFRunLoopStop`, which a pump would ignore). Carbon has one
//! event queue per process and binds it to the first thread that pumps
//! it, so a process has one pump (tests run each pumping check in a child
//! process of its own).
//!
//! The ids stay Windows virtual-key codes and `MOD_*` bits (`keymap`); this
//! module translates them: arrows to `kVK_*Arrow`, letters through the
//! current keyboard layout (`UCKeyTranslate`, so a letter is the key that
//! types it, as on Windows), digits and the rest by position (ANSI).
//!
//! Live checks against this Mac's hotkeys and media apps are `#[ignore]`
//! tests in `tests/mac_live.rs`; `tests/mac_seams.rs` checks the pump, the
//! routing, the timeout and the SIGTERM path without touching the user's.
use crate::hotkeys::ERROR_HOTKEY_ALREADY_REGISTERED;
use crate::keymap::{MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use crate::mac_nowplaying::{NowPlayingMedia, Snapshot};
use crate::platform::{
    AudioSessions, KeyState, KeyboardLayout, MediaSession, MediaSessions, OnSessionEnd, PResult,
    Platform, Registrar, TIMED_OUT,
};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The real platform.
pub fn platform() -> Platform {
    Platform {
        name: "macos",
        duck: false,
        audio: Arc::new(|| Box::new(NoDuck) as Box<dyn AudioSessions>),
        media: Arc::new(|| Box::new(MacMedia::new()) as Box<dyn MediaSessions>),
        registrar: Arc::new(|| registrar_with(Box::new(Carbon))),
        layout: Arc::new(NoAltGr),
        keys: Arc::new(Keys),
        session_end: Arc::new(watch_session_end),
    }
}

// ---- Carbon, Core Foundation and libdispatch ----------------------------

type OSStatus = i32;
type Ref = *mut c_void;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandler = extern "C" fn(Ref, Ref, *mut c_void) -> OSStatus;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> Ref;
    fn GetEventDispatcherTarget() -> Ref;
    fn InstallEventHandler(
        target: Ref,
        handler: EventHandler,
        count: usize,
        types: *const EventTypeSpec,
        user: *mut c_void,
        out: *mut Ref,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: Ref,
        options: u32,
        out: *mut Ref,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hotkey: Ref) -> OSStatus;
    fn GetEventParameter(
        event: Ref,
        name: u32,
        desired: u32,
        actual: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
    fn SetEventParameter(
        event: Ref,
        name: u32,
        kind: u32,
        size: usize,
        data: *const c_void,
    ) -> OSStatus;
    fn GetEventKind(event: Ref) -> u32;
    fn CreateEvent(
        allocator: Ref,
        class: u32,
        kind: u32,
        when: f64,
        flags: u32,
        out: *mut Ref,
    ) -> OSStatus;
    fn PostEventToQueue(queue: Ref, event: Ref, priority: i16) -> OSStatus;
    fn GetMainEventQueue() -> Ref;
    fn ReceiveNextEvent(
        count: usize,
        types: *const EventTypeSpec,
        timeout: f64,
        pull: u8,
        out: *mut Ref,
    ) -> OSStatus;
    fn SendEventToEventTarget(event: Ref, target: Ref) -> OSStatus;
    fn ReleaseEvent(event: Ref);
    fn GetCurrentEventLoop() -> Ref;
    fn QuitEventLoop(event_loop: Ref) -> OSStatus;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> Ref;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> Ref;
    fn TISGetInputSourceProperty(source: Ref, key: Ref) -> Ref;
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard: u32,
        options: u32,
        dead: *mut u32,
        max: usize,
        len: *mut usize,
        chars: *mut u16,
    ) -> OSStatus;
    fn LMGetKbdType() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: Ref;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
    fn CFRelease(object: Ref);
}

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_sync_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

const fn four(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const K_EVENT_CLASS_KEYBOARD: u32 = four(b"keyb");
const K_EVENT_HOTKEY_PRESSED: u32 = 5;
const K_EVENT_HOTKEY_RELEASED: u32 = 6;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = four(b"----");
const TYPE_EVENT_HOTKEY_ID: u32 = four(b"hkid");
const EVENT_NOT_HANDLED: OSStatus = -9874;
const EVENT_LOOP_QUIT: OSStatus = -9876;
const EVENT_HOTKEY_EXISTS: OSStatus = -9878;
/// The signature of Sonara's hotkey ids ('SNRA').
const SIGNATURE: u32 = four(b"SNRA");

/// Carbon modifier bits.
const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;

/// What `register` fails with when no thread runs the event pump (the
/// hotkeys could never fire).
pub const NO_EVENT_PUMP: u32 = 1;
/// What `register` fails with for a key this module cannot translate.
pub const UNKNOWN_KEY: u32 = 87;

fn is_main_thread() -> bool {
    // SAFETY: a plain query.
    unsafe { libc::pthread_main_np() == 1 }
}

// ---- the event pump -----------------------------------------------------

/// True while the main thread runs the pump (`run_main`): main-thread
/// calls can be queued to it.
static MAIN_PUMPING: AtomicBool = AtomicBool::new(false);

/// The Carbon event pump of one thread: dispatches every Carbon event
/// (hotkeys) and runs the thread's run loop until stopped.
pub struct Pump {
    stop: Arc<AtomicBool>,
    event_loop: usize,
}

/// Ends a `Pump::run` from any thread.
#[derive(Clone)]
pub struct PumpStopper {
    stop: Arc<AtomicBool>,
    event_loop: usize,
}

impl PumpStopper {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // SAFETY: QuitEventLoop may be called from any thread; the loop
        // lives as long as its thread.
        unsafe {
            QuitEventLoop(self.event_loop as Ref);
        }
    }
}

impl Pump {
    /// A pump for the calling thread.
    pub fn new() -> Pump {
        Pump {
            stop: Arc::new(AtomicBool::new(false)),
            // SAFETY: a plain query.
            event_loop: unsafe { GetCurrentEventLoop() } as usize,
        }
    }

    pub fn stopper(&self) -> PumpStopper {
        PumpStopper {
            stop: self.stop.clone(),
            event_loop: self.event_loop,
        }
    }

    /// Dispatch events until stopped. Installs the hotkey handler first.
    /// A stop before the first wait is not lost: the wait is bounded and
    /// the flag checked each time.
    pub fn run(&self) {
        install_handler();
        while !self.stop.load(Ordering::SeqCst) {
            let mut event: Ref = std::ptr::null_mut();
            // SAFETY: the event is ours to release once dispatched.
            unsafe {
                let r = ReceiveNextEvent(0, std::ptr::null(), 0.25, 1, &mut event);
                if r == EVENT_LOOP_QUIT {
                    continue;
                }
                if r == 0 && !event.is_null() {
                    SendEventToEventTarget(event, GetEventDispatcherTarget());
                    ReleaseEvent(event);
                }
            }
        }
    }
}

impl Default for Pump {
    fn default() -> Self {
        Pump::new()
    }
}

/// Run `f` on a worker thread while the calling thread runs the event pump
/// (call it from `main`, which must be the process's main thread: Carbon
/// delivers hotkeys only there). Returns `f`'s result once it returned; a
/// panic in `f` is passed on. Off the main thread it just runs `f`.
pub fn run_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    if !is_main_thread() {
        return f();
    }
    let pump = Pump::new();
    let stopper = pump.stopper();
    MAIN_PUMPING.store(true, Ordering::SeqCst);
    struct Done(PumpStopper);
    impl Drop for Done {
        fn drop(&mut self) {
            MAIN_PUMPING.store(false, Ordering::SeqCst);
            self.0.stop();
        }
    }
    let worker = std::thread::Builder::new()
        .name("main-worker".into())
        // The main thread's stack size.
        .stack_size(8 << 20)
        .spawn(move || {
            let _done = Done(stopper);
            f()
        })
        .expect("spawn the main worker thread");
    pump.run();
    match worker.join() {
        Ok(t) => t,
        Err(p) => std::panic::resume_unwind(p),
    }
}

struct Call<F, R> {
    f: Option<F>,
    r: Option<R>,
}

extern "C" fn trampoline<F: FnOnce() -> R, R>(context: *mut c_void) {
    // SAFETY: `context` is the `Call` that `on_main` keeps alive across
    // the synchronous dispatch.
    let call = unsafe { &mut *(context as *mut Call<F, R>) };
    if let Some(f) = call.f.take() {
        call.r = Some(f());
    }
}

/// Run `f` on the main thread and return its result: at once on the main
/// thread, else through the main dispatch queue while the pump runs.
/// `None` when no pump runs (it would never run).
fn on_main<F: FnOnce() -> R + Send, R: Send>(f: F) -> Option<R> {
    if is_main_thread() {
        return Some(f());
    }
    if !MAIN_PUMPING.load(Ordering::SeqCst) {
        return None;
    }
    let mut call = Call {
        f: Some(f),
        r: None,
    };
    // SAFETY: `call` outlives the synchronous dispatch; the main queue is
    // a static of libdispatch.
    unsafe {
        dispatch_sync_f(
            &_dispatch_main_q as *const c_void,
            &mut call as *mut Call<F, R> as *mut c_void,
            trampoline::<F, R>,
        );
    }
    call.r
}

// ---- hotkey routing -----------------------------------------------------

enum Msg {
    Press(i32),
    Stop,
}

struct Route {
    tx: Sender<Msg>,
    /// The registrar's id (`Action::id`).
    action: i32,
    /// The internal (Windows) key code, for `Keys`.
    vk: u32,
}

fn routes() -> MutexGuard<'static, HashMap<u32, Route>> {
    static ROUTES: OnceLock<Mutex<HashMap<u32, Route>>> = OnceLock::new();
    ROUTES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// The keys of hotkeys held down now (internal key codes), from the
/// pressed and released events (#299: a hold is told from a tap by the
/// release, not by polling the keyboard, which needs a permission).
fn pressed() -> MutexGuard<'static, HashSet<u32>> {
    static PRESSED: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    PRESSED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Hand a hotkey event to its registrar; false when the id is not one of
/// ours (any longer).
fn deliver(id: u32, down: bool) -> bool {
    let routes = routes();
    let Some(r) = routes.get(&id) else {
        return false;
    };
    if down {
        pressed().insert(r.vk);
        let _ = r.tx.send(Msg::Press(r.action));
    } else {
        pressed().remove(&r.vk);
    }
    true
}

extern "C" fn on_hotkey(_call: Ref, event: Ref, _user: *mut c_void) -> OSStatus {
    let mut hid = EventHotKeyID {
        signature: 0,
        id: 0,
    };
    // SAFETY: reads the event's hotkey id into a local of the right size.
    let (status, kind) = unsafe {
        (
            GetEventParameter(
                event,
                K_EVENT_PARAM_DIRECT_OBJECT,
                TYPE_EVENT_HOTKEY_ID,
                std::ptr::null_mut(),
                std::mem::size_of::<EventHotKeyID>(),
                std::ptr::null_mut(),
                &mut hid as *mut EventHotKeyID as *mut c_void,
            ),
            GetEventKind(event),
        )
    };
    if status != 0 || hid.signature != SIGNATURE {
        return EVENT_NOT_HANDLED;
    }
    let down = match kind {
        K_EVENT_HOTKEY_PRESSED => true,
        K_EVENT_HOTKEY_RELEASED => false,
        _ => return EVENT_NOT_HANDLED,
    };
    if deliver(hid.id, down) {
        0
    } else {
        EVENT_NOT_HANDLED
    }
}

/// The hotkey handler on the application target, once per process.
fn install_handler() {
    static INSTALLED: OnceLock<OSStatus> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let types = [
            EventTypeSpec {
                class: K_EVENT_CLASS_KEYBOARD,
                kind: K_EVENT_HOTKEY_PRESSED,
            },
            EventTypeSpec {
                class: K_EVENT_CLASS_KEYBOARD,
                kind: K_EVENT_HOTKEY_RELEASED,
            },
        ];
        // SAFETY: a static handler on the application target, never
        // removed; the type list is copied by the call.
        unsafe {
            InstallEventHandler(
                GetApplicationEventTarget(),
                on_hotkey,
                types.len(),
                types.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    });
}

/// Post a hotkey event to the main event queue, as the window server does
/// for a real press (a test seam: no key is touched). `id` is the id the
/// backend was given at registration.
pub fn post_hotkey(id: u32, down: bool) -> bool {
    let kind = if down {
        K_EVENT_HOTKEY_PRESSED
    } else {
        K_EVENT_HOTKEY_RELEASED
    };
    let hid = EventHotKeyID {
        signature: SIGNATURE,
        id,
    };
    // SAFETY: a new event owned here, released after the queue retained it.
    unsafe {
        let mut event: Ref = std::ptr::null_mut();
        if CreateEvent(
            std::ptr::null_mut(),
            K_EVENT_CLASS_KEYBOARD,
            kind,
            0.0,
            0,
            &mut event,
        ) != 0
        {
            return false;
        }
        SetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOTKEY_ID,
            std::mem::size_of::<EventHotKeyID>(),
            &hid as *const EventHotKeyID as *const c_void,
        );
        let ok = PostEventToQueue(GetMainEventQueue(), event, 1) == 0;
        ReleaseEvent(event);
        ok
    }
}

// ---- key translation ----------------------------------------------------

/// Carbon modifier bits of a `MOD_*` mask (`MOD_NOREPEAT` is dropped:
/// Carbon hotkeys do not repeat).
pub fn carbon_modifiers(mods: u32) -> u32 {
    let mut out = 0;
    for (m, c) in [
        (MOD_CTRL, CONTROL_KEY),
        (MOD_ALT, OPTION_KEY),
        (MOD_SHIFT, SHIFT_KEY),
        (MOD_WIN, CMD_KEY),
    ] {
        if mods & m != 0 {
            out |= c;
        }
    }
    out
}

/// The ANSI (US) key code of a character: the key at that position.
fn ansi_code(c: char) -> Option<u16> {
    Some(match c {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '6' => 0x16,
        '5' => 0x17,
        '9' => 0x19,
        '7' => 0x1A,
        '8' => 0x1C,
        '0' => 0x1D,
        ']' => 0x1E,
        'o' => 0x1F,
        'u' => 0x20,
        '[' => 0x21,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        'k' => 0x28,
        'n' => 0x2D,
        'm' => 0x2E,
        '.' => 0x2F,
        _ => return None,
    })
}

/// The macOS key code of an internal (Windows) key code. Letters come from
/// `layout` (the key that types the letter on the current layout, as a
/// Windows letter code does), falling back to the ANSI position; digits,
/// brackets and the period go by position, as Windows codes them.
pub fn key_code(vk: u32, layout: &dyn Fn(char) -> Option<u16>) -> Option<u16> {
    Some(match vk {
        0x26 => 0x7E, // up
        0x28 => 0x7D, // down
        0x25 => 0x7B, // left
        0x27 => 0x7C, // right
        0x24 => 0x73, // home
        0x23 => 0x77, // end
        0x21 => 0x74, // page up
        0x22 => 0x79, // page down
        0xBE => return ansi_code('.'),
        0xDB => return ansi_code('['),
        0xDD => return ansi_code(']'),
        0x30..=0x39 => return ansi_code(char::from_u32(vk)?),
        0x41..=0x5A => {
            let c = char::from_u32(vk)?.to_ascii_lowercase();
            return layout(c).or_else(|| ansi_code(c));
        }
        _ => return None,
    })
}

/// The current keyboard layout's key for each character it types without
/// modifiers (main thread only: Text Input Sources asserts it). An input
/// source without key layout data (some input methods) falls back to the
/// ASCII-capable layout.
fn layout_map() -> HashMap<char, u16> {
    let mut map = HashMap::new();
    // SAFETY: Text Input Sources calls on the main thread; the copied
    // source is released, the layout data is borrowed while it lives.
    unsafe {
        let mut source = TISCopyCurrentKeyboardLayoutInputSource();
        let mut data = if source.is_null() {
            std::ptr::null_mut()
        } else {
            TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        };
        if data.is_null() {
            if !source.is_null() {
                CFRelease(source);
            }
            source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
            if source.is_null() {
                return map;
            }
            data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        }
        if !data.is_null() {
            let layout = CFDataGetBytePtr(data) as *const c_void;
            let keyboard = u32::from(LMGetKbdType());
            for code in 0u16..0x80 {
                let mut dead = 0u32;
                let mut len = 0usize;
                let mut chars = [0u16; 4];
                // kUCKeyActionDisplay (3), no dead-key state (option bit 0).
                let r = UCKeyTranslate(
                    layout,
                    code,
                    3,
                    0,
                    keyboard,
                    1,
                    &mut dead,
                    chars.len(),
                    &mut len,
                    chars.as_mut_ptr(),
                );
                if r != 0 || len != 1 {
                    continue;
                }
                if let Some(c) = char::from_u32(u32::from(chars[0])) {
                    map.entry(c.to_ascii_lowercase()).or_insert(code);
                }
            }
        }
        CFRelease(source);
    }
    map
}

// ---- the registrar ------------------------------------------------------

/// Registers chords with the OS (a seam: the real one is Carbon on the
/// main thread; tests record instead).
pub trait Backend: Send {
    /// Register `vk` + `mods` (internal codes, `MOD_NOREPEAT` dropped) under
    /// the hotkey id `id`; returns a token for `unregister`, or the
    /// registrar error code.
    fn register(&mut self, vk: u32, mods: u32, id: u32) -> Result<usize, u32>;
    fn unregister(&mut self, token: usize);
}

/// The Carbon backend: every call on the main thread.
struct Carbon;

/// The registrar error code of a Carbon status: 1409 (another program or
/// another of our hotkeys holds it) for `eventHotKeyExistsErr`.
pub fn carbon_error(status: i32) -> u32 {
    if status == EVENT_HOTKEY_EXISTS {
        ERROR_HOTKEY_ALREADY_REGISTERED
    } else {
        status.unsigned_abs()
    }
}

impl Backend for Carbon {
    fn register(&mut self, vk: u32, mods: u32, id: u32) -> Result<usize, u32> {
        on_main(move || {
            install_handler();
            let map = layout_map();
            let code = key_code(vk, &|c| map.get(&c).copied()).ok_or(UNKNOWN_KEY)?;
            let mut hotkey: Ref = std::ptr::null_mut();
            // SAFETY: Carbon on the main thread; the ref is kept until
            // `unregister`.
            let status = unsafe {
                RegisterEventHotKey(
                    u32::from(code),
                    carbon_modifiers(mods),
                    EventHotKeyID {
                        signature: SIGNATURE,
                        id,
                    },
                    GetApplicationEventTarget(),
                    0,
                    &mut hotkey,
                )
            };
            if status != 0 {
                return Err(carbon_error(status));
            }
            Ok(hotkey as usize)
        })
        .unwrap_or(Err(NO_EVENT_PUMP))
    }

    fn unregister(&mut self, token: usize) {
        // Without a pump (the program is ending) the hotkey goes with the
        // process.
        let _ = on_main(move || {
            // SAFETY: a ref `register` returned, unregistered once.
            unsafe { UnregisterEventHotKey(token as Ref) };
        });
    }
}

/// Global hotkeys on `backend`; presses arrive through the event pump.
pub fn registrar_with(backend: Box<dyn Backend>) -> Box<dyn Registrar> {
    static SERIAL: AtomicU32 = AtomicU32::new(1);
    let (tx, rx) = mpsc::channel();
    Box::new(MacRegistrar {
        backend,
        serial: SERIAL.fetch_add(1, Ordering::SeqCst),
        tx,
        rx,
        held: HashMap::new(),
    })
}

struct MacRegistrar {
    backend: Box<dyn Backend>,
    /// Makes this registrar's hotkey ids unique in the process.
    serial: u32,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Registrar id -> (hotkey id, backend token).
    held: HashMap<i32, (u32, usize)>,
}

impl MacRegistrar {
    fn hotkey_id(&self, id: i32) -> u32 {
        (self.serial << 8) | (id as u32 & 0xFF)
    }
}

impl Registrar for MacRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, vk: u32) -> Result<(), u32> {
        let hid = self.hotkey_id(id);
        // The route first: a press right after registration is not lost.
        routes().insert(
            hid,
            Route {
                tx: self.tx.clone(),
                action: id,
                vk,
            },
        );
        match self.backend.register(vk, modifiers & 0xF, hid) {
            Ok(token) => {
                self.held.insert(id, (hid, token));
                Ok(())
            }
            Err(e) => {
                routes().remove(&hid);
                Err(e)
            }
        }
    }

    fn unregister(&mut self, id: i32) {
        if let Some((hid, token)) = self.held.remove(&id) {
            self.backend.unregister(token);
            if let Some(r) = routes().remove(&hid) {
                pressed().remove(&r.vk);
            }
        }
    }

    fn wait(&mut self) -> Option<i32> {
        match self.rx.recv() {
            Ok(Msg::Press(id)) => Some(id),
            Ok(Msg::Stop) | Err(_) => None,
        }
    }

    fn stopper(&self) -> Box<dyn Fn() + Send> {
        let tx = self.tx.clone();
        Box::new(move || {
            let _ = tx.send(Msg::Stop);
        })
    }
}

impl Drop for MacRegistrar {
    fn drop(&mut self) {
        let ids: Vec<i32> = self.held.keys().copied().collect();
        for id in ids {
            self.unregister(id);
        }
    }
}

/// Whether a hotkey's key is held, from its pressed and released events.
/// Only the keys of registered hotkeys are known; that is all `watch`
/// asks for.
pub struct Keys;

impl KeyState for Keys {
    fn is_down(&self, vk: u32) -> bool {
        pressed().contains(&vk)
    }
}

/// macOS has no AltGr: Option chords are hotkeys, and the character
/// Option types is the key's, never taken by a Ctrl+Option hotkey.
struct NoAltGr;

impl KeyboardLayout for NoAltGr {
    fn altgr_char(&self, _vk: u32, _shift: bool) -> Option<String> {
        None
    }
}

// ---- ducking: unavailable -------------------------------------------------

/// What listing audio sessions fails with on macOS.
pub const NO_DUCK: &str = "ducking is not available on macOS";

struct NoDuck;

impl AudioSessions for NoDuck {
    fn sessions(&self) -> PResult<Vec<Box<dyn crate::platform::AudioSession>>> {
        Err(NO_DUCK.into())
    }
}

// ---- media pause through AppleScript --------------------------------------

/// How long one AppleScript call may take (#16): an app that hangs, or the
/// first Automation permission prompt, must not hold the audio worker.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// The media apps paused through AppleScript, by bundle id: the scriptable
/// players with a `player state`. Every one of them is paused when it
/// plays, which Now Playing (one app at a time) cannot do, so Now Playing
/// leaves them to this path. Podcasts has no scripting dictionary (`sdef`
/// fails) and browsers are not scriptable for media: Now Playing reaches
/// them (`mac_nowplaying`).
pub const MEDIA_APPS: &[&str] = &["com.apple.Music", "com.spotify.client"];

/// Run an `osascript` script (`javascript` selects JavaScript for
/// Automation) and return its output, trimmed. Killed past `limit`, it
/// fails with `TIMED_OUT` (the app may still act on it later).
pub fn osascript(script: &str, javascript: bool, limit: Duration) -> PResult<String> {
    let mut cmd = Command::new("/usr/bin/osascript");
    if javascript {
        cmd.args(["-l", "JavaScript"]);
    }
    let mut child = cmd
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run osascript: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{TIMED_OUT} after {} ms", limit.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("osascript: {e}")),
        }
    };
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    if status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(format!("osascript: {}", err.trim()))
    }
}

/// The running apps among `MEDIA_APPS`. Asks AppKit, not the apps: no
/// Apple event, no permission prompt, and an app that is not installed is
/// no error.
fn running_media_apps() -> PResult<Vec<String>> {
    let ids = serde_json::to_string(MEDIA_APPS).unwrap_or_default();
    let script = format!(
        "ObjC.import('AppKit'); JSON.stringify({ids}.filter(id => \
         $.NSRunningApplication.runningApplicationsWithBundleIdentifier(id).count > 0))"
    );
    let out = osascript(&script, true, CALL_TIMEOUT)?;
    serde_json::from_str(&out).map_err(|e| format!("osascript answered {out:?}: {e}"))
}

/// One running player. Every call checks the app still runs first, so a
/// command never launches a player the user quit.
struct AppleScriptApp(String);

/// What a script answers for an app that no longer runs.
const GONE: &str = "gone";

/// The end of the error `AppleScriptApp::ask` returns for a gone app.
const NOT_RUNNING: &str = " is not running";

/// Read a `playerState` answer. Only a gone app is an error; a slow or
/// failed query is "not playing" (see `is_playing`).
fn playing_from(answer: PResult<String>) -> PResult<bool> {
    match answer {
        Ok(state) => Ok(state == "playing"),
        Err(e) if e.ends_with(NOT_RUNNING) => Err(e),
        Err(_) => Ok(false),
    }
}

impl AppleScriptApp {
    fn ask(&self, what: &str) -> PResult<String> {
        // The bundle id is one of `MEDIA_APPS` (no quoting needed).
        let script = format!(
            "var a = Application('{}'); a.running() ? String({what}) : '{GONE}'",
            self.0
        );
        let out = osascript(&script, true, CALL_TIMEOUT)?;
        if out == GONE {
            return Err(format!("{}{NOT_RUNNING}", self.0));
        }
        Ok(out)
    }
}

impl MediaSession for AppleScriptApp {
    fn app_id(&self) -> PResult<String> {
        Ok(self.0.clone())
    }

    /// `Err` means the app is gone, as `MediaPauser::resume` reads it. A
    /// slow or failed query (often the first Automation prompt timing out)
    /// is "not playing", so resume still tries `play`, which never launches
    /// a player the user quit: never leave an app paused.
    fn is_playing(&self) -> PResult<bool> {
        playing_from(self.ask("a.playerState()"))
    }

    fn pause(&self) -> PResult<()> {
        self.ask("(a.pause(), 'ok')").map(|_| ())
    }

    fn play(&self) -> PResult<()> {
        self.ask("(a.play(), 'ok')").map(|_| ())
    }
}

/// The media sessions of a Mac: the running AppleScript players
/// (`MEDIA_APPS`) and the Now Playing app (`mac_nowplaying`), listed in
/// parallel (one `osascript` each, both bounded by `CALL_TIMEOUT`). Fails
/// only when neither could be listed, so one path that is unavailable
/// never stops the other.
pub struct MacMedia {
    now: NowPlayingMedia,
}

impl MacMedia {
    pub fn new() -> Self {
        MacMedia {
            now: NowPlayingMedia::new(),
        }
    }
}

impl Default for MacMedia {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaSessions for MacMedia {
    fn sessions(&self) -> PResult<Vec<Box<dyn MediaSession>>> {
        let (apps, now) = std::thread::scope(|s| {
            let apps = s.spawn(running_media_apps);
            let now = self.now.read();
            let apps = apps
                .join()
                .unwrap_or_else(|_| Err("listing the media apps panicked".into()));
            (apps, now)
        });
        merge_sessions(apps, now, &self.now)
    }
}

/// The sessions of both paths; an error only when both failed.
fn merge_sessions(
    apps: PResult<Vec<String>>,
    now: PResult<Option<Snapshot>>,
    np: &NowPlayingMedia,
) -> PResult<Vec<Box<dyn MediaSession>>> {
    if let (Err(a), Err(n)) = (&apps, &now) {
        return Err(format!("{a}; Now Playing: {n}"));
    }
    let mut out: Vec<Box<dyn MediaSession>> = match apps {
        Ok(ids) => ids
            .into_iter()
            .map(|id| Box::new(AppleScriptApp(id)) as Box<dyn MediaSession>)
            .collect(),
        Err(e) => {
            eprintln!("sonara-system: [pause] cannot list Music and Spotify: {e}");
            Vec::new()
        }
    };
    out.extend(np.sessions_from(now));
    Ok(out)
}

// ---- the session end ------------------------------------------------------

/// The callbacks to run when the session ends.
fn end_callbacks() -> MutexGuard<'static, Vec<OnSessionEnd>> {
    static CALLBACKS: OnceLock<Mutex<Vec<OnSessionEnd>>> = OnceLock::new();
    CALLBACKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// After SIGTERM and the callbacks, how long the program has to end by
/// itself (its own SIGTERM handling exits cleanly) before it is ended.
static EXIT_GRACE_MS: AtomicU64 = AtomicU64::new(10_000);

/// Set how long a SIGTERM leaves the program to exit by itself after the
/// session-end callbacks ran (default 10 s; a test seam).
pub fn set_exit_grace(d: Duration) {
    EXIT_GRACE_MS.store(d.as_millis() as u64, Ordering::SeqCst);
}

/// The write end of the self-pipe the signal handler and the power-off
/// observer wake the watcher through.
static WAKE_FD: OnceLock<i32> = OnceLock::new();

/// A byte on the pipe: SIGTERM (end the process after the callbacks) or
/// the power-off notification (callbacks only: SIGTERM follows).
const WAKE_TERM: u8 = b'T';
const WAKE_POWER_OFF: u8 = b'P';

fn wake(byte: u8) {
    if let Some(fd) = WAKE_FD.get() {
        // SAFETY: write(2) is async-signal-safe; one byte from a local.
        unsafe {
            libc::write(*fd, &byte as *const u8 as *const c_void, 1);
        }
    }
}

/// Run `f` when the session ends (logout, shutdown, #18): on SIGTERM (what
/// launchd sends every process of the session at logout) and on
/// `NSWorkspaceWillPowerOffNotification` (posted to apps; a background
/// process may never see it, SIGTERM is the path it relies on). The
/// callbacks run once, on a thread of their own. SIGTERM is caught through
/// the registry tokio uses too, so a host's own SIGTERM handling still
/// runs; since a caught SIGTERM no longer ends the process by itself, it
/// is ended once the callbacks are done and the exit grace has passed.
pub fn watch_session_end(f: OnSessionEnd) {
    end_callbacks().push(f);
    static STARTED: OnceLock<bool> = OnceLock::new();
    STARTED.get_or_init(|| {
        let ok = start_watcher();
        if !ok {
            eprintln!("sonara-system: cannot watch for the end of the session");
        }
        ok
    });
}

fn start_watcher() -> bool {
    let mut fds = [0i32; 2];
    // SAFETY: a new pipe; both ends stay open for the life of the process.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return false;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    // SAFETY: close-on-exec on our own descriptors, so children never
    // inherit them.
    unsafe {
        libc::fcntl(read_fd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(write_fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let _ = WAKE_FD.set(write_fd);
    let spawned = std::thread::Builder::new()
        .name("sonara-system-session".into())
        .spawn(move || {
            let ran = AtomicBool::new(false);
            loop {
                let mut byte = 0u8;
                // SAFETY: a blocking read of one byte into a local.
                let n = unsafe { libc::read(read_fd, &mut byte as *mut u8 as *mut c_void, 1) };
                if n != 1 {
                    continue;
                }
                if !ran.swap(true, Ordering::SeqCst) {
                    // Taken out so a callback can never block another
                    // registration; they run once.
                    let callbacks = std::mem::take(&mut *end_callbacks());
                    for f in &callbacks {
                        f();
                    }
                }
                if byte == WAKE_TERM {
                    let grace = Duration::from_millis(EXIT_GRACE_MS.load(Ordering::SeqCst));
                    std::thread::sleep(grace);
                    std::process::exit(128 + libc::SIGTERM);
                }
            }
        })
        .is_ok();
    if !spawned {
        return false;
    }
    // SAFETY: the action only writes one byte to a pipe (async-signal-safe).
    let registered = unsafe { signal_hook_registry::register(libc::SIGTERM, || wake(WAKE_TERM)) };
    if registered.is_err() {
        return false;
    }
    observe_power_off();
    true
}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Ref;
    fn sel_registerName(name: *const c_char) -> Ref;
    fn objc_allocateClassPair(superclass: Ref, name: *const c_char, extra: usize) -> Ref;
    fn objc_registerClassPair(class: Ref);
    fn class_addMethod(class: Ref, sel: Ref, imp: *const c_void, types: *const c_char) -> bool;
    fn objc_msgSend();
}

extern "C" fn on_power_off(_this: Ref, _cmd: Ref, _note: Ref) {
    wake(WAKE_POWER_OFF);
}

/// Observe `NSWorkspaceWillPowerOffNotification` (AppKit, loaded here so
/// a process that never watches the session end does not load it).
fn observe_power_off() {
    // SAFETY: Objective-C runtime calls with the signatures of the methods
    // sent (objc_msgSend cast per call, as arm64 requires); the observer
    // and its class live for the life of the process.
    unsafe {
        let appkit = libc::dlopen(
            c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(),
            libc::RTLD_LAZY,
        );
        if appkit.is_null() {
            return;
        }
        let name =
            libc::dlsym(appkit, c"NSWorkspaceWillPowerOffNotification".as_ptr()) as *const Ref;
        let workspace_class = objc_getClass(c"NSWorkspace".as_ptr());
        let object_class = objc_getClass(c"NSObject".as_ptr());
        if name.is_null() || workspace_class.is_null() || object_class.is_null() {
            return;
        }
        let class = objc_allocateClassPair(object_class, c"SonaraPowerOffObserver".as_ptr(), 0);
        if class.is_null() {
            return;
        }
        let selector = sel_registerName(c"powerOff:".as_ptr());
        class_addMethod(
            class,
            selector,
            on_power_off as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(class);
        let send0: extern "C" fn(Ref, Ref) -> Ref = std::mem::transmute(objc_msgSend as *const ());
        let add: extern "C" fn(Ref, Ref, Ref, Ref, Ref, Ref) =
            std::mem::transmute(objc_msgSend as *const ());
        let workspace = send0(
            workspace_class,
            sel_registerName(c"sharedWorkspace".as_ptr()),
        );
        let center = send0(workspace, sel_registerName(c"notificationCenter".as_ptr()));
        let observer = send0(
            send0(class, sel_registerName(c"alloc".as_ptr())),
            sel_registerName(c"init".as_ptr()),
        );
        if center.is_null() || observer.is_null() {
            return;
        }
        add(
            center,
            sel_registerName(c"addObserver:selector:name:object:".as_ptr()),
            observer,
            selector,
            *name,
            std::ptr::null_mut(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_whose_state_query_times_out_is_still_resumed() {
        let timed_out = Err(format!("{TIMED_OUT} after 2000 ms"));
        assert_eq!(playing_from(timed_out), Ok(false));
        assert_eq!(playing_from(Err("osascript: -1743".into())), Ok(false));
    }

    fn chrome() -> PResult<Option<Snapshot>> {
        Ok(Some(Snapshot {
            app: "com.google.Chrome".into(),
            title: "A video".into(),
            state: 1,
            is_playing: true,
        }))
    }

    fn no_run() -> NowPlayingMedia {
        NowPlayingMedia::with(
            std::sync::Arc::new(|_: &str| Err("not called".into())),
            "",
            MEDIA_APPS,
        )
    }

    fn ids(s: &[Box<dyn MediaSession>]) -> Vec<String> {
        s.iter().map(|s| s.app_id().unwrap()).collect()
    }

    #[test]
    fn both_paths_are_listed_together() {
        let np = no_run();
        let s = merge_sessions(Ok(vec!["com.apple.Music".into()]), chrome(), &np).unwrap();
        assert_eq!(ids(&s), ["com.apple.Music", "com.google.Chrome"]);
    }

    #[test]
    fn now_playing_unavailable_falls_back_to_applescript() {
        let np = no_run();
        let s = merge_sessions(
            Ok(vec!["com.spotify.client".into()]),
            Err("MediaRemote did not load".into()),
            &np,
        )
        .unwrap();
        assert_eq!(ids(&s), ["com.spotify.client"]);
        let s = merge_sessions(Err(format!("{TIMED_OUT} after 2000 ms")), chrome(), &np).unwrap();
        assert_eq!(ids(&s), ["com.google.Chrome"]);
    }

    #[test]
    fn listing_fails_only_when_both_paths_fail() {
        let np = no_run();
        let e = merge_sessions(Err("a".into()), Err("b".into()), &np)
            .err()
            .unwrap();
        assert!(e.contains('a') && e.contains('b'), "{e}");
    }

    #[test]
    fn a_player_that_quit_is_not_resumed() {
        let gone = Err(format!("com.apple.Music{NOT_RUNNING}"));
        assert!(playing_from(gone).is_err());
        assert_eq!(playing_from(Ok("playing".into())), Ok(true));
        assert_eq!(playing_from(Ok("paused".into())), Ok(false));
    }
}
