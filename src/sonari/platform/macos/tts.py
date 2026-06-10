"""macOS TTS backend — wraps the `say` command."""
from __future__ import annotations

import subprocess
from typing import List, Optional, Tuple

from sonari.platform.base import TtsBackend


def _parse_listing(listing: str) -> "List[Tuple[str, str, bool]]":
    """Parse ``say -v ?`` output into (bare_name, locale, is_premium) triples.

    Each output line has the form::

        Voice Name (Enhanced)   en_US  # some sample text

    The hash and everything after it is a sample phrase that we discard.
    We split the portion before the hash into tokens; the last token is the
    locale code, everything preceding it is the voice name (possibly
    including a ``(Premium)`` / ``(Enhanced)`` qualifier).
    """
    results: "List[Tuple[str, str, bool]]" = []
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
        results.append((bare, locale, is_premium))
    return results


class MacTtsBackend(TtsBackend):
    def run(self, text: str, voice: Optional[str], rate: int):
        cmd = ["say"]
        if voice:
            cmd += ["-v", voice]
        cmd += ["-r", str(rate), text]
        return subprocess.Popen(cmd)

    def list_voices(self) -> "List[str]":
        """Return all installed voice names (bare, without qualifier tags)."""
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return []
        return [bare for bare, _locale, _premium in _parse_listing(listing)]

    def best_voice(self) -> str:
        """Return the best English voice: Premium/Enhanced first, then
        Allison > Samantha as plain-English fallbacks, then ``"Samantha"``
        hard-coded as the last resort."""
        fallback = "Samantha"
        try:
            listing = subprocess.check_output(["say", "-v", "?"], text=True)
        except (FileNotFoundError, OSError, subprocess.SubprocessError):
            return fallback
        premium_en: "List[str]" = []
        plain_en: "List[str]" = []
        for bare, locale, is_premium in _parse_listing(listing):
            if not locale.startswith("en"):
                continue
            (premium_en if is_premium else plain_en).append(bare)
        if premium_en:
            return premium_en[0]
        for preferred in ("Allison", "Samantha"):
            if preferred in plain_en:
                return preferred
        return fallback
