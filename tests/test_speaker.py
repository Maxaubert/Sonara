import subprocess
import threading

from echo.speaker import Speaker


class FakePopen:
    """Stand-in for subprocess.Popen exposing wait()/terminate()."""

    def __init__(self):
        self.wait_calls = 0
        self.terminate_calls = 0

    def wait(self, timeout=None):
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

        def wait(self, timeout=None):
            # While we are "blocking", the speaker treats us as current.
            self._speaker.cancel()
            return super().wait(timeout=timeout)

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


class FakeEarconPopen:
    """Fake Popen returned by an earcon player; controllable finish state."""

    def __init__(self, finished: bool = False):
        self._finished = finished
        self.poll_calls = 0

    def finish(self):
        self._finished = True

    def poll(self):
        self.poll_calls += 1
        return 0 if self._finished else None


class RecordingEarconPlayer:
    """earcon_player that returns FakeEarconPopen instances for tracking."""

    def __init__(self):
        self.paths = []
        self.procs: list[FakeEarconPopen] = []

    def __call__(self, path: str) -> FakeEarconPopen:
        self.paths.append(path)
        proc = FakeEarconPopen()
        self.procs.append(proc)
        return proc


def test_finished_earcon_processes_are_reaped_on_next_earcon():
    """Finished earcon procs must be removed from _earcon_procs on the next call."""
    player = RecordingEarconPlayer()
    earcons = {
        "ping": "/sounds/ping.aiff",
        "pong": "/sounds/pong.aiff",
        "done": "/sounds/done.aiff",
    }
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)

    # Fire three earcons; all three procs are still running.
    sp.earcon("ping")
    sp.earcon("pong")
    sp.earcon("done")
    assert len(sp._earcon_procs) == 3

    # Mark the first two as finished.
    player.procs[0].finish()
    player.procs[1].finish()

    # The next earcon call must reap the two finished procs before adding the new one.
    sp.earcon("ping")
    # Only the still-running third proc plus the newly spawned proc should remain.
    assert len(sp._earcon_procs) == 2
    # The remaining procs are the previously-running third and the brand-new fourth.
    assert sp._earcon_procs[0] is player.procs[2]
    assert sp._earcon_procs[1] is player.procs[3]


def test_earcon_procs_do_not_accumulate_unbounded():
    """Finished procs must be removed so the list stays bounded (no zombie buildup)."""
    player = RecordingEarconPlayer()
    earcons = {"tick": "/sounds/tick.aiff"}
    sp = Speaker(say_runner=RecordingRunner(), earcon_player=player, earcons=earcons)

    N = 20
    for i in range(N):
        # Immediately mark the previous proc as done before firing the next one.
        if player.procs:
            player.procs[-1].finish()
        sp.earcon("tick")

    # Every finished proc must have been reaped; at most 1 (the last) can remain.
    assert len(sp._earcon_procs) <= 1


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


# ---------------------------------------------------------------------------
# Lock / race-condition / timeout tests
# ---------------------------------------------------------------------------


def test_cancel_terminates_proc_tracked_as_current():
    """cancel() must call terminate() on the proc held in _current."""
    terminated = []
    captured_speaker = {}

    class TrackedPopen(FakePopen):
        def wait(self, timeout=None):
            # The proc is current at this point; cancel the speaker.
            captured_speaker["sp"].cancel()
            return super().wait(timeout=timeout)

        def terminate(self):
            terminated.append(True)
            super().terminate()

    class TrackedRunner:
        def __call__(self, text, voice, rate):
            return TrackedPopen()

    sp = Speaker(say_runner=TrackedRunner())
    captured_speaker["sp"] = sp
    sp.speak("test")
    # terminate() must have been called once from cancel().
    assert len(terminated) == 1


def test_speak_wait_timeout_terminates_hung_proc():
    """A proc whose wait() always raises TimeoutExpired must be terminated."""

    class HungPopen(FakePopen):
        def wait(self, timeout=None):
            if timeout is not None:
                raise subprocess.TimeoutExpired(cmd=["say"], timeout=timeout)
            # No-timeout call should never happen in normal flow.
            return 0

    class HungRunner:
        def __call__(self, text, voice, rate):
            return HungPopen()

    # Inject a very small timeout (0.01 s) so the test is instantaneous.
    sp = Speaker(say_runner=HungRunner(), _wait_timeout=0.01)
    sp.speak("will hang")
    # The fake proc's terminate must have been called.
    # We verify by checking terminate_calls via a shared reference.
    procs = []

    class TrackingHungPopen(FakePopen):
        def wait(self, timeout=None):
            if timeout is not None:
                raise subprocess.TimeoutExpired(cmd=["say"], timeout=timeout)
            return 0

    class TrackingHungRunner:
        def __call__(self, text, voice, rate):
            p = TrackingHungPopen()
            procs.append(p)
            return p

    sp2 = Speaker(say_runner=TrackingHungRunner(), _wait_timeout=0.01)
    sp2.speak("will hang tracked")
    assert len(procs) == 1
    assert procs[0].terminate_calls == 1
