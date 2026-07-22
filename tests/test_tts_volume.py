"""Speech-volume gain: pure WAV scaling, no winsound needed (cross-platform)."""
import array
import io
import wave

from sonara.platform.windows import tts


def _wav16(samples, framerate=24000):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(framerate)
        w.writeframes(array.array("h", samples).tobytes())
    return buf.getvalue()


def _samples(data):
    with wave.open(io.BytesIO(data), "rb") as r:
        out = array.array("h")
        out.frombytes(r.readframes(r.getnframes()))
    return list(out)


def test_100_percent_is_identity_bytes():
    data = _wav16([0, 1000, -1000, 32767])
    assert tts._scale_wav(data, 100) is data


def test_50_percent_halves_samples():
    data = _wav16([0, 1000, -1000, 20000])
    assert _samples(tts._scale_wav(data, 50)) == [0, 500, -500, 10000]


def test_200_percent_doubles_and_clamps():
    data = _wav16([0, 1000, -1000, 20000, -20000])
    assert _samples(tts._scale_wav(data, 200)) == [0, 2000, -2000, 32767, -32768]


def test_8bit_wav_passes_through():
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(1)
        w.setframerate(8000)
        w.writeframes(b"\x80\x90\xa0")
    data = buf.getvalue()
    assert tts._scale_wav(data, 50) == data


def test_malformed_bytes_pass_through():
    junk = b"not a wav at all"
    assert tts._scale_wav(junk, 50) == junk


def test_set_volume_clamps_and_get_reports():
    tts.set_volume(150)
    assert tts.get_volume() == 150
    tts.set_volume(999)
    assert tts.get_volume() == 200
    tts.set_volume(1)
    assert tts.get_volume() == 25
    tts.set_volume("junk")
    assert tts.get_volume() == 25          # unchanged on junk
    tts.set_volume(100)                    # restore for other tests
