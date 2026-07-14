"""Pre-rendered voice previews (#38): the settings page must play a preview
INSTANTLY from a file; live synthesis (seconds on Chatterbox) is fallback only.
"""
import pytest

from sonara import previews


def test_preview_path_is_per_voice_and_filesystem_safe(tmp_path, monkeypatch):
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    p1 = previews.preview_path("af_heart")
    p2 = previews.preview_path("Microsoft Zira")
    assert p1.name == "af_heart.wav"
    assert p2.parent == tmp_path
    assert "/" not in p2.name and "\\" not in p2.name
    # hostile names cannot escape the dir
    p3 = previews.preview_path("..\\..\\evil")
    assert p3.parent == tmp_path


def test_sample_text_mentions_the_voice():
    assert "af_heart" in previews.sample_text("af_heart")


def test_ensure_all_generates_only_missing(tmp_path, monkeypatch):
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    (tmp_path / "af_heart.wav").write_bytes(b"RIFFexisting")
    made = []

    def fake_synth(voice):
        made.append(voice)
        return b"RIFFnew"
    n = previews.ensure_all({"kokoro": ["af_heart", "af_bella"],
                             "windows": ["Microsoft Zira"]}, synth=fake_synth)
    assert sorted(made) == ["Microsoft Zira", "af_bella"]   # af_heart skipped
    assert n == 2
    assert (tmp_path / "af_bella.wav").read_bytes() == b"RIFFnew"
    assert (tmp_path / "af_heart.wav").read_bytes() == b"RIFFexisting"


def test_ensure_all_survives_synth_failures(tmp_path, monkeypatch):
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)

    def flaky(voice):
        if voice == "bad":
            raise RuntimeError("engine down")
        return b"RIFFok"
    n = previews.ensure_all({"kokoro": ["bad", "good"]}, synth=flaky)
    assert n == 1                                           # 'good' still made
    assert (tmp_path / "good.wav").exists()
    assert not (tmp_path / "bad.wav").exists()              # no empty/corrupt file


def test_ensure_all_ignores_empty_synth_output(tmp_path, monkeypatch):
    monkeypatch.setattr(previews, "preview_dir", lambda: tmp_path)
    n = previews.ensure_all({"kokoro": ["v"]}, synth=lambda v: b"")
    assert n == 0
    assert not (tmp_path / "v.wav").exists()
