"""Platform backend interfaces. The portable core depends ONLY on these
abstractions; the concrete Windows implementation lives in a sibling package
and is wired in by get_platform() (the only sys.platform guard for backend
SELECTION; transport.py branches separately for its stdlib lock primitive)."""
from __future__ import annotations

import abc
from dataclasses import dataclass


class TtsBackend(abc.ABC):
    @abc.abstractmethod
    def run(self, text: str, voice, rate: int, on_play=None):
        """Start speaking *text*; return a proc-like handle exposing
        .wait(timeout=None), .terminate(), and .returncode (0 == completed).
        This is the say_runner the Speaker orchestrates. *on_play*, when given,
        MUST be invoked at playback start (after synthesis): the daemon hooks
        audio ducking there so other apps' audio dips when sound begins, not
        through a multi-second synthesis."""

    @abc.abstractmethod
    def best_voice(self) -> str:
        """Return the best installed voice name (a sensible default)."""

    @abc.abstractmethod
    def list_voices(self) -> "list[str]":
        """Return installed voice names (may be empty)."""


class EarconBackend(abc.ABC):
    @abc.abstractmethod
    def play(self, path: str):
        """Play the sound at *path* non-blocking; return a proc-like handle
        exposing .poll(), or None on error/missing file."""

    @abc.abstractmethod
    def default_earcons(self) -> "dict":
        """Return the platform's default {kind: sound_path} mapping."""


class HotkeyBackend(abc.ABC):
    @abc.abstractmethod
    def install(self) -> "tuple":
        """Set up the global-hotkey mechanism. Return (ok: bool, detail: str).

        On Windows the hotkeys run in-process and are started by the daemon, so
        there is nothing to provision here."""

    @abc.abstractmethod
    def uninstall(self) -> None:
        """Tear down the global-hotkey mechanism."""

    @abc.abstractmethod
    def display_combo(self, modifiers: int, key_code: int) -> str:
        """Human label for a (modifiers, key_code) pair, e.g. 'Ctrl+Cmd+O'."""

    # --- keytables (consumed by the portable keymap resolver) ---
    def key_codes(self) -> "dict":
        """Map key-name -> OS key code for this platform."""
        return {}

    def mod_masks(self) -> "dict":
        """Map modifier-name -> OS modifier mask for this platform."""
        return {}

    def default_mods(self) -> "list":
        """The platform's default modifier chord (e.g. ['ctrl','cmd'])."""
        return []

    # --- in-process lifecycle (Windows runs the listener on a daemon thread) ---
    def start(self, dispatch) -> None:
        """Begin listening for global hotkeys. *dispatch* is callable(message: dict)
        invoked on each fire. Default: no-op."""
        return None

    def stop(self) -> None:
        """Stop listening. Default: no-op."""
        return None

    def reload(self, dispatch) -> None:
        """Re-apply the current keymap to the live listener after keymap.json
        changed. Default: a full stop()+start() cycle, which is the Windows
        in-process reload path -- stop() releases the live chords before start()
        re-registers the updated keymap on a fresh pump thread."""
        self.stop()
        self.start(dispatch)

    def doctor_rows(self) -> "list":
        """Platform hotkey diagnostics (collisions, integrity). Default: none."""
        return []


class SupervisorBackend(abc.ABC):
    @abc.abstractmethod
    def install(self, python: str, app_dir: str) -> None: ...
    @abc.abstractmethod
    def uninstall(self) -> None: ...
    @abc.abstractmethod
    def is_running(self) -> bool: ...
    @abc.abstractmethod
    def is_installed(self) -> bool:
        """Cheap check the user ran `sonara install` (the launcher/agent exists)."""
    @abc.abstractmethod
    def resolve_python(self): ...
    @abc.abstractmethod
    def launch_spec(self) -> "tuple":
        """Return (argv, spawn_kwargs) to lazily start the daemon process."""
    @abc.abstractmethod
    def doctor_rows(self) -> "list":
        """Return platform-specific [(name, ok, detail), ...] diagnostic rows."""

    # Concrete defaults (overridden per platform) so existing subclasses and test
    # doubles keep working without implementing them.
    def post_install_notes(self) -> None:
        """Print OS-specific post-install next steps. Default: nothing."""
        return None

    def hooks_doctor_row(self) -> "tuple":
        """Return a (name, ok, detail) row describing whether Sonara's hooks are
        installed. Default: unknown."""
        return ("hooks installed", False, "unknown")


@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
    ducker: object = None     # AudioDucker/NullDucker; duck-typed (duck/restore/is_ducked)
    pauser: object = None     # MediaPauser/NullPauser; duck-typed (pause/resume/is_paused)
