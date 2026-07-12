import os
import sys

import pytest
from sonara import paths, chatterbox_provision as cbp


def test_chatterbox_requirements_file_pins_chatterbox_tts():
    text = open(cbp.chatterbox_requirements_path()).read()
    assert "chatterbox-tts==0.1.7" in text


def test_provision_creates_venv_and_installs(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "CHATTERBOX_VENV", tmp_path / "venv")
    monkeypatch.setattr(paths, "chatterbox_venv_python",
                        lambda: str(tmp_path / "venv" / "Scripts" / "python.exe"))
    cmds = []
    cbp.provision("/bin/uv", run=lambda cmd, **k: cmds.append(cmd))
    assert cmds[0] == ["/bin/uv", "venv", str(tmp_path / "venv"), "--python", "3.12"]
    # torch AND torchaudio must both come from the cu128 index (deviation from
    # the brief: see docs/superpowers/specs/2026-07-12-chatterbox-smoke.md -
    # a bare `chatterbox.tts` import fails without a cu128 torchaudio).
    assert cmds[1][:4] == ["/bin/uv", "pip", "install", "--python"]
    assert "torch" in cmds[1] and "torchaudio" in cmds[1]
    assert cmds[1][-2:] == ["--index-url", cbp._TORCH_INDEX]
    assert cmds[2][:4] == ["/bin/uv", "pip", "install", "--python"]
    assert "-r" in cmds[2] and cbp.chatterbox_requirements_path() in cmds[2]


def test_warmup_runs_worker_ping(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "chatterbox_venv_python", lambda: "/venv/Scripts/python.exe")
    monkeypatch.setattr(paths, "CHATTERBOX_HF_CACHE", tmp_path / "hf-cache")
    seen = {}

    def fake_run(cmd, env=None, **k):
        seen["cmd"], seen["env"] = cmd, env

    cbp.warmup(run=fake_run)
    assert seen["cmd"][0] == "/venv/Scripts/python.exe"
    assert seen["env"]["HF_HOME"] == str(tmp_path / "hf-cache")
    assert "PYTHONPATH" in seen["env"]
    assert "handle_request" in seen["cmd"][-1]


def test_uninstall_removes_venv_and_cache(monkeypatch, tmp_path):
    venv = tmp_path / "venv"
    cache = tmp_path / "hf-cache"
    venv.mkdir()
    cache.mkdir()
    monkeypatch.setattr(paths, "CHATTERBOX_VENV", venv)
    monkeypatch.setattr(paths, "CHATTERBOX_HF_CACHE", cache)
    removed = []
    cbp.uninstall_chatterbox(rmtree=lambda p: removed.append(p))
    assert str(venv) in removed and str(cache) in removed


def test_uninstall_is_idempotent_when_absent(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "CHATTERBOX_VENV", tmp_path / "no-venv")
    monkeypatch.setattr(paths, "CHATTERBOX_HF_CACHE", tmp_path / "no-cache")
    cbp.uninstall_chatterbox(rmtree=lambda p: pytest.fail("must not rmtree a missing dir"))


def test_install_chatterbox_runs_steps_in_order():
    order = []
    cbp.install_chatterbox(
        ensure_uv=lambda **k: order.append("uv") or "/bin/uv",
        provision=lambda uv, **k: order.append(("provision", uv)),
        warmup=lambda **k: order.append("warmup"),
    )
    assert order == ["uv", ("provision", "/bin/uv"), "warmup"]


def test_install_chatterbox_aborts_if_provision_fails():
    def boom(uv, **k):
        raise RuntimeError("uv venv failed")
    with pytest.raises(RuntimeError):
        cbp.install_chatterbox(
            ensure_uv=lambda **k: "/bin/uv",
            provision=boom,
            warmup=lambda **k: pytest.fail("must not warm up"),
        )
