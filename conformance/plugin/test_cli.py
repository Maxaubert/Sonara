"""sonara.exe (#202): the plugin's settings, doctor, start, stop and
uninstall commands, against temporary homes and the fake engine."""
from __future__ import annotations

import json
import re
import sys
import urllib.request

import pytest

import plugin_harness as ph


@pytest.fixture
def cli(box, exes):
    def run(*args, timeout=60.0):
        return box.cli(exes / "sonara.exe", *args, timeout=timeout)
    return run


def test_settings_starts_the_runtime_and_prints_the_page_with_its_token(box, cli):
    p = cli("settings")
    assert p.returncode == 0, p.stdout + p.stderr
    m = re.search(r"(http://127\.0\.0\.1:\d+/settings\?token=[0-9a-f]+)", p.stdout)
    assert m, p.stdout
    info = box.runtime_info()
    assert info and f":{info['http_port']}/" in m.group(1)
    assert info["token"] in m.group(1)
    with urllib.request.urlopen(m.group(1), timeout=10) as r:
        assert r.status == 200
        assert "Sonara" in r.read().decode("utf-8")


def test_doctor_reports_rows_and_never_fails_on_information(box, cli):
    p = cli("doctor")
    assert p.returncode == 0, p.stdout + p.stderr
    rows = p.stdout.splitlines()
    assert all(re.match(r"\[( OK |INFO|WARN|FAIL)\] ", r) for r in rows), rows
    text = p.stdout
    assert f"[INFO] version: sonara {ph.VERSION}" in text
    assert f"[INFO] home: {box.home}" in text
    assert re.search(r"\[ OK \] runtime: started now, version " + re.escape(ph.VERSION), text)
    assert "[ OK ] engine: fake, ready" in text
    assert "250 words per minute" in text, "the product default rate"
    assert "[INFO] audio: media apps are paused" in text, "the product default audio mode"
    assert re.search(r"\[ OK \] hotkeys: Ctrl\+Alt\+Up restart", text)
    assert "[ OK ] hooks:" in text, "run from the plugin (CLAUDE_PLUGIN_ROOT)"
    assert "[WARN] onnxruntime.dll" in text or "[ OK ] onnxruntime.dll" in text
    assert "[FAIL]" not in text


def test_doctor_warns_about_the_python_sonara_left_behind(box, cli):
    (box.profile / ".sonara" / "app").mkdir(parents=True)
    p = cli("doctor")
    assert p.returncode == 0
    assert "[WARN] python sonara:" in p.stdout


def test_stop_keeps_sonara_off_and_start_brings_it_back(box, cli):
    assert cli("start").returncode == 0
    pid = box.runtime_info()["pid"]
    p = cli("stop")
    assert p.returncode == 0, p.stdout + p.stderr
    assert f"Sonara stopped (pid {pid})" in p.stdout
    assert (box.home / "stopped").is_file()
    assert box.runtime_info() is None, "the runtime exited and removed runtime.json"
    p = cli("doctor")
    assert p.returncode == 0
    assert "[WARN] runtime: stopped; run /sonara:start" in p.stdout
    p = cli("start")
    assert p.returncode == 0
    assert "started" in p.stdout
    assert not (box.home / "stopped").exists()
    p = cli("start")
    assert "already running" in p.stdout


def test_uninstall_removes_all_but_the_keep_list_and_stays_off(box, cli, exes):
    box.install(exes, ph.VERSION)
    assert cli("start").returncode == 0
    for f in ("config.json", "keymap.json"):
        (box.home / f).write_text("{}", encoding="utf-8")
    (box.home / "models" / "kokoro").mkdir(parents=True)
    (box.home / "logs").mkdir(exist_ok=True)
    p = cli("uninstall", "--keep", "settings")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "Stopped Sonara" in p.stdout
    assert f"Removed {box.root}" in p.stdout
    assert f"Kept {box.home / 'config.json'}" in p.stdout
    assert not box.root.exists()
    assert not (box.home / "models").exists() and not (box.home / "logs").exists()
    assert (box.home / "config.json").is_file() and (box.home / "keymap.json").is_file()
    assert (box.home / "stopped").is_file(), "the hooks stay quiet until /sonara:start"
    assert "/plugin uninstall sonara@sonara" in p.stdout


def test_uninstall_refuses_an_unknown_keep_item(cli):
    p = cli("uninstall", "--keep", "everything")
    assert p.returncode == 2
    assert "unknown --keep item" in p.stderr


def test_engines_add_command_writes_engines_json_and_the_runtime_reloads(box, cli):
    """A program is never sent over the protocol (sonarad refuses it): the
    CLI writes engines.json as the user, then asks for engine_reload."""
    argv = [sys.executable, "-c", "pass", "a & b | c > d %PATH%"]
    p = cli("engines", "add", "say", "--kind", "command", "--option", f"argv={json.dumps(argv)}")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "Added say: it runs on this PC" in p.stdout
    stored = json.loads((box.home / "engines.json").read_text(encoding="utf-8"))
    assert stored["engines"][0]["options"]["argv"] == argv, "stored literally"
    p = cli("engines", "list")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "say (" in p.stdout and "runs on this PC" in p.stdout
    p = cli("engines", "add", "say", "--kind", "command", "--option", f"argv={json.dumps(argv)}")
    assert p.returncode == 1 and "exists; add --replace" in p.stderr
    # An entry the runtime cannot use is taken out of the file again.
    bad = [sys.executable, "{in}"]
    p = cli("engines", "add", "bad", "--kind", "command", "--option", f"argv={json.dumps(bad)}")
    assert p.returncode == 1, p.stdout
    assert "needs option input 'file'" in p.stderr
    stored = json.loads((box.home / "engines.json").read_text(encoding="utf-8"))
    assert [e["id"] for e in stored["engines"]] == ["say"]
    p = cli("engines", "remove", "say")
    assert p.returncode == 0, p.stdout + p.stderr
