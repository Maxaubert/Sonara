from echo.speaker import Speaker


class FakePopen:
    """Stand-in for subprocess.Popen exposing wait()/terminate()."""

    def __init__(self):
        self.wait_calls = 0
        self.terminate_calls = 0

    def wait(self):
        self.wait_calls += 1
        return 0

    def terminate(self):
        self.terminate_calls += 1


class RecordingRunner:
    """Records say_runner(text, voice, rate) calls; returns a fresh FakePopen each time."""

    def __init__(self):
        self.calls = []
        self.procs = []

    def __call__(self, text, voice, rate):
        proc = FakePopen()
        self.calls.append((text, voice, rate))
        self.procs.append(proc)
        return proc


def test_speak_calls_say_runner_with_voice_rate_and_blocks_on_wait():
    runner = RecordingRunner()
    sp = Speaker(voice="Ava", rate=180, say_runner=runner)
    sp.speak("hello world")
    assert runner.calls == [("hello world", "Ava", 180)]
    assert runner.procs[0].wait_calls == 1


def test_speak_tracks_current_proc():
    runner = RecordingRunner()
    sp = Speaker(say_runner=runner)
    sp.speak("one")
    sp.speak("two")
    assert len(runner.procs) == 2
    assert runner.procs[0].wait_calls == 1
    assert runner.procs[1].wait_calls == 1


def test_cancel_after_speak_completes_is_noop():
    runner = RecordingRunner()

    # A runner whose returned proc does NOT auto-finish on wait(): we drive
    # cancel() before the (simulated) blocking wait by calling speak in a way
    # that lets us inspect the tracked proc. Here we use a runner that records
    # the proc and lets us cancel after wait returns, then assert terminate.
    sp = Speaker(say_runner=runner)
    sp.speak("blah")
    # After speak returns, current proc is cleared; cancel must be a safe no-op.
    sp.cancel()
    assert runner.procs[0].terminate_calls == 0


def test_cancel_terminates_active_proc_mid_speak():
    # Use a runner whose proc.wait() invokes a hook so we can cancel while the
    # proc is still tracked as current.
    captured = {}

    class CancelOnWaitPopen(FakePopen):
        def __init__(self, speaker):
            super().__init__()
            self._speaker = speaker

        def wait(self):
            # While we are "blocking", the speaker treats us as current.
            self._speaker.cancel()
            return super().wait()

    class HookRunner:
        def __init__(self):
            self.procs = []

        def __call__(self, text, voice, rate):
            proc = CancelOnWaitPopen(captured["speaker"])
            self.procs.append(proc)
            return proc

    runner = HookRunner()
    sp = Speaker(say_runner=runner)
    captured["speaker"] = sp
    sp.speak("active")
    assert runner.procs[0].terminate_calls == 1


def test_cancel_with_no_current_proc_is_noop():
    sp = Speaker(say_runner=RecordingRunner())
    # Never called speak; cancel must not raise.
    sp.cancel()


class RecordingEarcon:
    def __init__(self):
        self.paths = []

    def __call__(self, path):
        self.paths.append(path)


def test_earcon_plays_mapped_path():
    player = RecordingEarcon()
    earcons = {
        "permission": "/System/Library/Sounds/Funk.aiff",
        "choice": "/System/Library/Sounds/Ping.aiff",
    }
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)
    sp.earcon("choice")
    assert player.paths == ["/System/Library/Sounds/Ping.aiff"]


def test_earcon_unknown_kind_is_noop():
    player = RecordingEarcon()
    earcons = {"choice": "/System/Library/Sounds/Ping.aiff"}
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)
    sp.earcon("does-not-exist")
    assert player.paths == []


def test_earcon_kind_with_no_mapping_is_noop():
    player = RecordingEarcon()
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons={})
    sp.earcon("choice")
    assert player.paths == []


import echo.speaker as speaker_mod


def test_play_earcon_missing_file_is_tolerated(monkeypatch):
    called = {"popen": False}

    def fake_popen(args):
        called["popen"] = True
        return object()

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: False)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    # Missing file: must return without spawning afplay and without raising.
    speaker_mod.play_earcon("/no/such/sound.aiff")
    assert called["popen"] is False


def test_play_earcon_spawn_error_is_tolerated(monkeypatch):
    def fake_popen(args):
        raise FileNotFoundError("afplay missing")

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    # Binary missing: must not raise.
    speaker_mod.play_earcon("/System/Library/Sounds/Tink.aiff")


def test_play_earcon_invokes_afplay_with_path(monkeypatch):
    recorded = {}

    def fake_popen(args):
        recorded["args"] = args
        return object()

    monkeypatch.setattr(speaker_mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(speaker_mod.subprocess, "Popen", fake_popen)
    speaker_mod.play_earcon("/System/Library/Sounds/Tink.aiff")
    assert recorded["args"] == ["afplay", "/System/Library/Sounds/Tink.aiff"]


from echo.speaker import best_enhanced_voice

SAY_SAMPLE = (
    "Albert              en_US    # Hello! My name is Albert.\n"
    "Alice               it_IT    # Ciao! Mi chiamo Alice.\n"
    "Allison             en_US    # Hi, my name is Allison.\n"
    "Ava (Premium)       en_US    # Hi, my name is Ava.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
    "Samantha            en_US    # Hi, my name is Samantha.\n"
    "Zoe (Premium)       en_US    # Hi, my name is Zoe.\n"
    "Zuzana              cs_CZ    # Dobrý den, jmenuji se Zuzana.\n"
)

SAY_SAMPLE_NO_PREMIUM = (
    "Albert              en_US    # Hello! My name is Albert.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
    "Zuzana              cs_CZ    # Dobrý den, jmenuji se Zuzana.\n"
)

SAY_SAMPLE_PREMIUM_NON_EN = (
    "Alice (Premium)     it_IT    # Ciao! Mi chiamo Alice.\n"
    "Daniel              en_GB    # Hello, my name is Daniel.\n"
)


def test_best_enhanced_voice_prefers_premium_en(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess, "check_output", lambda *a, **k: SAY_SAMPLE
    )
    voice = best_enhanced_voice()
    assert voice in ("Ava", "Zoe")
    # The first Premium en voice in the listing wins.
    assert voice == "Ava"


def test_best_enhanced_voice_falls_back_to_samantha_when_no_premium(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess,
        "check_output",
        lambda *a, **k: SAY_SAMPLE_NO_PREMIUM,
    )
    assert best_enhanced_voice() == "Samantha"


def test_best_enhanced_voice_ignores_premium_non_en(monkeypatch):
    monkeypatch.setattr(
        speaker_mod.subprocess,
        "check_output",
        lambda *a, **k: SAY_SAMPLE_PREMIUM_NON_EN,
    )
    # Premium voice is Italian; must fall back to Samantha.
    assert best_enhanced_voice() == "Samantha"


def test_best_enhanced_voice_falls_back_when_say_errors(monkeypatch):
    def boom(*a, **k):
        raise FileNotFoundError("say missing")

    monkeypatch.setattr(speaker_mod.subprocess, "check_output", boom)
    assert best_enhanced_voice() == "Samantha"
