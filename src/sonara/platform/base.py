"""Platform backend interfaces. The portable core depends ONLY on these
abstractions; the concrete Windows implementation lives in a sibling package
and is wired in by get_platform() (the only sys.platform guard in Sonara)."""
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

    def set_volume(self, percent) -> None:
        """Speech gain percent (25-200). Default: no-op (backend has no gain)."""
        return None

    def prewarm(self, rate: int) -> None:
        """Load the neural engine ahead of the first cue (#60), so that cue
        does not pay the engine load. Blocking; the daemon calls it on a
        background thread. Default: no-op (nothing to load)."""
        return None


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
    def install(self, python: str, app_dir: str,
                plugin_root: "str | None" = None) -> None:
        """Wire autostart, hooks and launcher. *plugin_root* is the plugin tree
        the hooks point at (None: the tree this code runs from)."""
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
    def end_task(self) -> None:
        """End the autostart-launched supervisor tree. Default: nothing."""
        return None

    def kill_stray_daemons(self) -> int:
        """After a stop, end any daemon process the SHUTDOWN message could not
        reach (#65). Returns how many were ended. Default: none."""
        return 0

    def post_install_notes(self) -> None:
        """Print OS-specific post-install next steps. Default: nothing."""
        return None

    def hooks_doctor_row(self) -> "tuple":
        """Return a (name, ok, detail) row describing whether Sonara's hooks are
        installed. Default: unknown."""
        return ("hooks installed", False, "unknown")


class NullDucker:
    """No-op ducker: the daemon default until the platform's ducker is
    injected (tests, a backend without ducking)."""

    def is_ducked(self) -> bool:
        return False

    def duck(self, exclude_pids, level: int) -> None:
        pass

    def restore(self) -> None:
        pass

    def recover(self) -> None:
        pass


class NullPauser:
    """No-op pauser: the daemon default until the platform's pauser is
    injected. Mirrors NullDucker."""

    def is_paused(self) -> bool:
        return False

    def pause(self) -> None:
        pass

    def resume(self) -> None:
        pass

    def recover(self) -> None:
        pass


@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
    # Duck-typed like NullDucker / NullPauser: duck/restore/is_ducked and
    # pause/resume/is_paused, plus recover() for the startup crash sweep.
    ducker: object = None
    pauser: object = None

    def recover_audio(self) -> None:
        """Daemon startup: undo any ducking or media pause a crashed earlier
        daemon left behind (never leave other apps ducked or paused)."""
        for part in (self.ducker, self.pauser):
            recover = getattr(part, "recover", None)
            if recover is not None:
                recover()
