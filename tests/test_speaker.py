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
