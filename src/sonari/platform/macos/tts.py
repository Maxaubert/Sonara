"""macOS TTS backend — wraps the `say` command."""
from __future__ import annotations

import subprocess

from sonari.platform.base import TtsBackend


class MacTtsBackend(TtsBackend):
    def run(self, text: str, voice, rate: int):
        cmd = ["say"]
        if voice:
            cmd += ["-v", voice]
        cmd += ["-r", str(rate), text]
        return subprocess.Popen(cmd)

    def list_voices(self) -> "list[str]":
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return []
        names = []
        for line in listing.splitlines():
            before_hash = line.split("#", 1)[0].rstrip()
            parts = before_hash.split()
            if len(parts) >= 2:
                names.append(" ".join(parts[:-1]))
        return names

    def best_voice(self) -> str:
        fallback = "Samantha"
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return fallback
        premium_en, plain_en = [], []
        for line in listing.splitlines():
            line = line.rstrip()
            if not line:
                continue
            before_hash = line.split("#", 1)[0].rstrip()
            parts = before_hash.split()
            if len(parts) < 2:
                continue
            locale = parts[-1]
            name = " ".join(parts[:-1])
            is_premium = "(Premium)" in name or "(Enhanced)" in name
            bare = name.replace("(Premium)", "").replace("(Enhanced)", "").strip()
            if not locale.startswith("en"):
                continue
            (premium_en if is_premium else plain_en).append(bare)
        if premium_en:
            return premium_en[0]
        for preferred in ("Allison", "Samantha"):
            if preferred in plain_en:
                return preferred
        return fallback
