import subprocess


def run_say(text: str, voice, rate: int):
    cmd = ["say"]
    if voice:
        cmd += ["-v", voice]
    cmd += ["-r", str(rate), text]
    return subprocess.Popen(cmd)


def play_earcon(path: str) -> None:
    try:
        subprocess.Popen(["afplay", path])
    except FileNotFoundError:
        pass


def best_enhanced_voice() -> str:
    return "Samantha"


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

    def set_voice(self, v) -> None:
        self._voice = v

    def set_rate(self, r) -> None:
        self._rate = r
