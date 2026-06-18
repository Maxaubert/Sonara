"""Platform backend interfaces. The portable core depends ONLY on these
abstractions; concrete macOS/Windows implementations live in sibling packages
and are wired in by get_platform() (the only sys.platform branch for backend
SELECTION; transport.py branches separately for its stdlib lock primitive)."""
from __future__ import annotations

import abc
from dataclasses import dataclass


class TtsBackend(abc.ABC):
    @abc.abstractmethod
    def run(self, text: str, voice, rate: int):
        """Start speaking *text*; return a proc-like handle exposing
        .wait(timeout=None), .terminate(), and .returncode (0 == completed).
        This is the say_runner the Speaker orchestrates."""

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
    def install(self, log_path: str, agent_path: str, launchctl_fn) -> "tuple":
        """Set up the global-hotkey mechanism. Return (ok: bool, detail: str).

        *log_path*     – path to the hotkey daemon log file.
        *agent_path*   – path where the LaunchAgent plist is written.
        *launchctl_fn* – callable(args) → int; abstracted so tests can patch it.
        """

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

    # --- in-process lifecycle (Windows runs a thread; macOS runs a process) ---
    def start(self, dispatch) -> None:
        """Begin listening for global hotkeys. *dispatch* is callable(message: dict)
        invoked on each fire. Default: no-op (macOS hotkeyd is a separate process)."""
        return None

    def stop(self) -> None:
        """Stop listening. Default: no-op."""
        return None

    def reload(self, dispatch) -> None:
        """Re-apply the current keymap to the live listener after keymap.json
        changed. Default: a full stop()+start() cycle (correct for an in-process
        listener like Windows, whose stop() releases its chords before start()
        re-registers them). Platforms whose hotkeys run in a SEPARATE process
        (macOS) override this to rewrite the resolved keymap and reload that
        process instead — stop()/start() are no-ops there."""
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
        """Cheap check the user ran `sonari install` (the launcher/agent exists)."""
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
        """Return a (name, ok, detail) row describing whether Sonari's hooks are
        installed. Default: unknown."""
        return ("hooks installed", False, "unknown")


@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
