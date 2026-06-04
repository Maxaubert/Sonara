from tests.daemon_helpers import FakeSpeaker, make_daemon


def test_fake_speaker_records():
    fs = FakeSpeaker()
    fs.speak("hi")
    fs.earcon("plan")
    fs.cancel()
    fs.set_rate(150)
    fs.set_voice("Ava")
    assert fs.spoken == ["hi"]
    assert fs.earcons == ["plan"]
    assert fs.cancels == 1
    assert fs.rates == [150]
    assert fs.voices == ["Ava"]


def test_make_daemon_wires_components():
    daemon, queue, speaker, sessions, config = make_daemon()
    assert sessions.foreground() == "fg"
    assert config["verbosity"] == "everything"
    assert len(queue) == 0
    assert isinstance(speaker, FakeSpeaker)
