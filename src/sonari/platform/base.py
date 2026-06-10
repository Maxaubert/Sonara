"""Platform backend interfaces. The portable core depends ONLY on these
abstractions; concrete macOS/Windows implementations live in sibling packages
and are wired in by get_platform() (the only sys.platform branch)."""
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
    def install(self) -> "tuple":
        """Set up the global-hotkey mechanism. Return (ok: bool, detail: str)."""

    @abc.abstractmethod
    def uninstall(self) -> None:
        """Tear down the global-hotkey mechanism."""

    @abc.abstractmethod
    def display_combo(self, modifiers: int, key_code: int) -> str:
        """Human label for a (modifiers, key_code) pair, e.g. 'Ctrl+Cmd+O'."""


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


@dataclass
class PlatformBackend:
    tts: TtsBackend
    earcon: EarconBackend
    hotkey: HotkeyBackend
    supervisor: SupervisorBackend
