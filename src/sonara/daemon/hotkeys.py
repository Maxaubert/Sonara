"""Global hotkeys in the daemon: start, stop and reload the platform
listener, debounce toggle double-taps, and hand each fire to a worker that
applies it under the daemon lock exactly like a socket message. Also says
so audibly when the hotkeys fail to start or another program holds them,
and handles RELOAD_KEYMAP."""
from __future__ import annotations

import os
import queue
import sys
import threading

from sonara.daemon import core
from sonara.protocol import MsgType

# Hotkey debounce: ignore a repeat of the SAME toggle within this window so an
# accidental/rapid double-tap doesn't flip pause/mute several times (and pile
# up confirmation cues). Directional keys (nav/repeat/skip) are NOT debounced --
# repeated presses there are intentional. NEXT_SESSION is directional too: each
# press is a deliberate ring advance (and now chimes instantly, #111), and
# MOD_NOREPEAT already guards key-hold auto-repeat, so it is not debounced.
DEBOUNCE_S = 0.30
DEBOUNCED_TYPES = (MsgType.PAUSE, MsgType.MUTE)


def hotkeys_disabled() -> bool:
    """The kill switch: SONARA_DISABLE_HOTKEYS, or a no_hotkeys file in the
    Sonara dir. Resolved through paths.SONARA_DIR at call time, like every
    other ~/.sonara path."""
    if os.environ.get("SONARA_DISABLE_HOTKEYS"):
        return True
    from sonara import paths
    return os.path.exists(str(paths.SONARA_DIR / "no_hotkeys"))


class HotkeyController:
    """Owns the hotkey worker queue, the debounce state and the reload lock.
    Given the daemon's shared state explicitly: *lock* is the daemon lock,
    *running* the daemon's running event, *handle_message* applies one
    message (called under *lock*), *cues* speaks the failure notices."""

    def __init__(self, lock, running, handle_message, cues) -> None:
        self._lock = lock
        self._running = running
        self._handle_message = handle_message
        self._cues = cues
        # Hotkey fires are handed to this queue by the Windows pump thread and
        # applied by a dedicated worker under the daemon lock -- so the pump NEVER
        # blocks on the lock and presses can't pile up then burst while the daemon
        # is busy streaming prose (the mute-hang). Drained by worker().
        self.queue: "queue.Queue" = queue.Queue()
        self._last: dict = {}                     # toggle type -> last fire (debounce)
        self._reload_lock = threading.Lock()      # serializes off-lock hotkey reloads
        self._failure_announced = False

    def register(self, table: dict) -> None:
        core.add_handlers(table, {MsgType.RELOAD_KEYMAP: self.on_reload_keymap})

    def on_reload_keymap(self, msg):
        # keymap.json changed: re-register off the daemon lock.
        self.request_reload()
        return None

    def dispatch(self, message: dict) -> None:
        """Called ON the Windows hotkey PUMP thread for each fire. It MUST NOT block:
        debounce (cheap, pump-thread-only state) then hand the message to the worker
        queue and return to GetMessage immediately. Running handle_message here
        (under the daemon lock) used to stall the pump whenever the daemon held the
        lock streaming prose, so presses queued at the OS level and burst later --
        the mute-hang. The worker applies the message under the lock."""
        import time as _t
        if self.debounce_suppress(message.get("type"), _t.monotonic()):
            return   # a too-fast repeat of the same toggle -> ignore
        self.queue.put(message)

    def worker(self) -> None:
        """Drain queued hotkey fires and apply each under the daemon lock -- OFF the
        pump thread, so a busy daemon can never stall hotkey CAPTURE. Serialized
        (single worker) like the old synchronous dispatch, and serialized against
        the socket path via the daemon lock."""
        while self._running.is_set():
            try:
                message = self.queue.get(timeout=0.2)
            except queue.Empty:
                continue
            if message is None:        # shutdown sentinel from stop_worker()
                break
            self.process(message)

    def stop_worker(self) -> None:
        """Unblock the worker's get() so it exits."""
        self.queue.put(None)

    def process(self, message: dict) -> None:
        """Apply one hotkey message exactly like an inbound socket message.

        MUST hold the daemon lock around handle_message, identical to the socket
        path (server.handle_conn): it mutates shared state (channels, history,
        config) concurrently with the speak loop, so without the lock it races ->
        'list changed size during iteration' / corruption. handle_message and its
        callees never acquire the lock (note_spoken/speak run on the speak thread),
        so this is deadlock-free. Contained so one bad hotkey can't kill the
        worker."""
        try:
            with self._lock:
                self._handle_message(message)
        except Exception:  # noqa: BLE001 - one bad hotkey must not kill the worker
            import traceback
            traceback.print_exc(file=sys.stderr)

    def debounce_suppress(self, mtype, now) -> bool:
        """True if *mtype* is a repeat of the same TOGGLE hotkey within the debounce
        window -- collapses an accidental/rapid double-tap into one action. Only the
        toggles in DEBOUNCED_TYPES are debounced; nav/repeat/skip pass through so
        repeated directional presses still register. Runs on the single hotkey pump
        thread, so the unlocked _last access is race-free."""
        if mtype not in DEBOUNCED_TYPES:
            return False
        last = self._last.get(mtype)
        if last is not None and (now - last) < DEBOUNCE_S:
            return True
        self._last[mtype] = now
        return False

    def start(self) -> None:
        """Start the platform's global-hotkey listener: an in-process
        RegisterHotKey thread."""
        # Kill-switch: a ~/.sonara/no_hotkeys file (or SONARA_DISABLE_HOTKEYS=1)
        # runs speech-only (no in-process hotkey thread). A FILE flag is honoured
        # by EVERY daemon however it is spawned (hooks inherit their own env, not
        # ours), so it reliably isolates the hotkey thread when diagnosing crashes.
        if hotkeys_disabled():
            return
        from sonara.platform import get_platform
        try:
            from sonara import keymap
            keymap.migrate_default_chord()   # one-time upgrade of the legacy chord
            backend = get_platform().hotkey
            backend.start(self.dispatch)
            self.announce_collisions(getattr(backend, "collisions", None))
        except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
            self.failed("start")

    def failed(self, what: str) -> None:
        """Log why the hotkeys did not start (traceback to the log) and say so
        once per run (M4): a bad keymap.json used to leave every hotkey dead
        with no sign at all. Called from an except block."""
        import traceback
        print("[hotkeys] {0} failed:".format(what), file=sys.stderr, flush=True)
        traceback.print_exc(file=sys.stderr)
        if self._failure_announced:
            return
        self._failure_announced = True
        # Off-lock caller (start/reload): the cue reslices CONTROL, take the lock.
        with self._lock:
            self._cues.speak(None, "Sonara hotkeys could not start. Run sonara "
                             "doctor to see why.", exempt_mute=True,
                             pause_exempt=True)

    def announce_collisions(self, collisions) -> None:
        """Surface failed RegisterHotKey chords AUDIBLY (#65). Windows grants a
        chord to ONE process: in a split-brain (a stray older daemon surviving a
        restart) the new daemon owns the socket but not the keys, so hotkey
        presses act on a daemon the user cannot hear about - mute appears
        broken. Collisions were only recorded for `sonara doctor`; an eyes-free
        user needs to HEAR that the keys went elsewhere."""
        if not collisions:
            return
        names = ", ".join(sorted(str(c.get("action", "?")) for c in collisions))
        print("[hotkeys] failed to register: {0}".format(names),
              file=sys.stderr, flush=True)
        with self._lock:                   # called off-lock from start()
            self._cues.speak(None,
                             "Some Sonara hotkeys are held by another program. "
                             "Restarting Sonara may fix it.",
                             exempt_mute=True, pause_exempt=True)

    def stop(self) -> None:
        from sonara.platform import get_platform
        try:
            get_platform().hotkey.stop()
        except Exception:  # noqa: BLE001 - shutdown must not raise
            pass

    def reload(self) -> None:
        """Apply a keymap.json change to the live hotkeys. Runs OFF the daemon lock
        (see request_reload) and is serialized by _reload_lock so two rapid
        reloads can't interleave their stop/start cycles. Honors the no_hotkeys
        kill switch, then delegates to the platform backend's reload() seam, a
        (thread-joined) stop+start."""
        with self._reload_lock:
            if hotkeys_disabled():
                self.stop()
                return
            from sonara.platform import get_platform
            try:
                get_platform().hotkey.reload(self.dispatch)
            except Exception:  # noqa: BLE001 - hotkeys are non-essential; speech must run
                self.failed("reload")

    def request_reload(self) -> None:
        """RELOAD_KEYMAP: keymap.json changed (e.g. an unbind): re-register hotkeys
        so it takes effect without a daemon restart. Run it OFF the daemon lock:
        this handler is invoked while holding the daemon lock, but reload() joins
        the Windows hotkey pump thread, which itself needs the lock to dispatch a
        fire. Joining under the lock could stall the daemon up to the join timeout
        and, on timeout, leave an orphaned thread that re-creates the H2
        dark-hotkey race. A short-lived thread does the reload lock-free (and
        _reload_lock serializes concurrent reloads)."""
        threading.Thread(target=self.reload,
                         name="sonara-keymap-reload", daemon=True).start()
