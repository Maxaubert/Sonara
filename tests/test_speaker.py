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


def test_cancel_terminates_the_current_proc():
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
