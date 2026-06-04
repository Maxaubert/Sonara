import os
import subprocess


def run_say(text: str, voice, rate: int):
    cmd = ["say"]
    if voice:
        cmd += ["-v", voice]
    cmd += ["-r", str(rate), text]
    return subprocess.Popen(cmd)


def play_earcon(path: str) -> None:
    if not os.path.exists(path):
        return
    try:
        subprocess.Popen(["afplay", path])
    except (FileNotFoundError, OSError):
        pass


def best_enhanced_voice() -> str:
    fallback = "Samantha"
    try:
        listing = subprocess.check_output(
            ["say", "-v", "?"], text=True
        )
    except (FileNotFoundError, OSError, subprocess.SubprocessError):
        return fallback

    premium_en = []
    plain_en = []
    for line in listing.splitlines():
        line = line.rstrip()
        if not line:
            continue
        # Format: "Name [maybe (Quality)] <pad> locale # sample"
        before_hash = line.split("#", 1)[0].rstrip()
        parts = before_hash.split()
        if len(parts) < 2:
            continue
        locale = parts[-1]
        name_tokens = parts[:-1]
        name = " ".join(name_tokens)
        is_premium = "(Premium)" in name or "(Enhanced)" in name
        # Bare display name without the quality suffix.
        bare = name.replace("(Premium)", "").replace("(Enhanced)", "").strip()
        if not locale.startswith("en"):
            continue
        if is_premium:
            premium_en.append(bare)
        else:
            plain_en.append(bare)

    if premium_en:
        return premium_en[0]
    for preferred in ("Allison", "Samantha"):
        if preferred in plain_en:
            return preferred
    return fallback


class Speaker:
    def __init__(
        self,
        voice=None,
        rate=200,
        say_runner=run_say,
        earcon_player=play_earcon,
        earcons=None,
    ) -> None:
        self._voice = voice
        self._rate = rate
        self._say_runner = say_runner
        self._earcon_player = earcon_player
        self._earcons = dict(earcons) if earcons else {}
        self._current = None

    def speak(self, text: str) -> None:
        proc = self._say_runner(text, self._voice, self._rate)
        self._current = proc
        proc.wait()
        self._current = None

    def cancel(self) -> None:
        proc = self._current
        if proc is not None:
            proc.terminate()

    def earcon(self, kind: str) -> None:
        path = self._earcons.get(kind)
        if path is None:
            return
        self._earcon_player(path)

    def set_voice(self, v) -> None:
        self._voice = v

    def set_rate(self, r) -> None:
        self._rate = r
