"""bin/sonara-hook-launch and bin/sonara-bootstrap.ps1 (#202): the plugin's
hooks run the runtime's sonara-hook.exe, and install the runtime from the
GitHub release first when it is missing, without ever blocking or failing
a Claude session."""
from __future__ import annotations

import json
import subprocess
import time

import plugin_harness as ph

# A hook may take at most about a second (Claude Code waits for it).
QUICK = 1.5
NO_START = {"SONARA_BOOTSTRAP_START": "0"}


def serve_good_release(box, releases, exes):
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(data)})
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    return r


def test_a_missing_runtime_is_installed_in_the_background(box, releases, exes):
    r = serve_good_release(box, releases, exes)
    code, took = box.launch("SessionStart", {"session_id": "s1"}, NO_START)
    assert code == 0
    assert took < QUICK, f"the hook waited {took:.2f} s for the bootstrap"
    box.wait_for(lambda: (box.dest / "sonara-hook.exe").is_file(), what="the install")
    box.wait_bootstrap()
    for e in ph.EXES:
        assert (box.dest / e).is_file(), e
    assert r.hits == {"SHA256SUMS": 1, ph.ZIP_NAME: 1}
    leftovers = [p.name for p in box.root.iterdir() if p.name.startswith(".")]
    assert leftovers == [], "no lock, staging folder or failure marker is left"


def test_a_checksum_mismatch_installs_nothing(box, releases, exes):
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(b"something else")})
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    code, took = box.launch("SessionStart", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    box.wait_for(lambda: r.hits.get(ph.ZIP_NAME), what="the download")
    box.wait_bootstrap()
    assert not box.dest.exists()
    assert (box.root / ".bootstrap.failed").is_file()
    assert "checksum mismatch" in box.log()
    assert not [p for p in box.root.iterdir() if p.name.startswith(".staging")]


def test_offline_the_hook_exits_at_once_and_waits_before_trying_again(box):
    box.env["SONARA_RELEASE_BASE_URL"] = ph.closed_port_url()
    code, took = box.launch("SessionStart", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    box.wait_for(lambda: (box.root / ".bootstrap.failed").is_file(), what="the failure marker")
    box.wait_bootstrap()
    assert not box.dest.exists()
    # The next hooks do not start another bootstrap for a while.
    code, took = box.launch("UserPromptSubmit", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    assert not (box.root / ".bootstrap.lock").exists(), "no new bootstrap"


def test_parallel_hooks_start_a_single_bootstrap(box, releases, exes):
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(data)}, delay=0.5)
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    env = dict(box.env, **NO_START)
    procs = [
        subprocess.Popen([str(ph.find_bash()), ph.posix(ph.BIN / "sonara-hook-launch"), ev],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         env=env, creationflags=0x08000000)
        for ev in ["SessionStart", "UserPromptSubmit", "MessageDisplay", "PreToolUse"] * 2
    ]
    for p in procs:
        p.communicate(b"{}", timeout=30)
        assert p.returncode == 0
    box.wait_for(lambda: (box.dest / "sonara-hook.exe").is_file(), what="the install")
    box.wait_bootstrap()
    assert r.hits.get(ph.ZIP_NAME) == 1, r.hits


def test_an_installed_runtime_gets_the_event_and_payload(box, releases, exes, tmp_path):
    r = serve_good_release(box, releases, exes)
    box.install(exes)
    capture = tmp_path / "capture"
    payload = json.loads((ph.REPO / "tests" / "fixtures" / "MessageDisplay.json")
                         .read_text(encoding="utf-8"))
    code, took = box.launch("MessageDisplay", payload,
                            {"SONARA_CAPTURE": str(capture), "SONARA_NO_START": "1"})
    assert code == 0
    assert took < QUICK
    (got,) = list(capture.glob("MessageDisplay-*.json"))
    assert json.loads(got.read_text(encoding="utf-8")) == payload, "stdin passed through"
    assert r.hits == {}, "nothing downloaded"
    assert not (box.root / ".bootstrap.lock").exists()


def test_an_upgrade_removes_the_old_runtime(box, releases, exes):
    serve_good_release(box, releases, exes)
    old = box.install(exes, "0.0.1")
    code, _ = box.launch("SessionStart", {}, NO_START)
    assert code == 0
    box.wait_for(lambda: (box.dest / "sonara-hook.exe").is_file(), what="the install")
    box.wait_bootstrap()
    assert not old.exists(), "only the current release is kept"
    assert "Removed the old runtime 0.0.1" in box.log()


def test_a_stopped_sonara_installs_nothing(box, releases, exes):
    r = serve_good_release(box, releases, exes)
    box.home.mkdir(parents=True)
    (box.home / "stopped").write_bytes(b"")
    code, took = box.launch("SessionStart", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    time.sleep(1.0)
    assert r.hits == {}
    assert not box.dest.exists()


def test_the_wrapper_installs_the_runtime_before_a_command(box, releases, exes):
    serve_good_release(box, releases, exes)
    p = box.wrapper("version")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "Installing the Sonara runtime" in p.stdout
    assert f"sonara {ph.VERSION}" in p.stdout
    assert (box.dest / "sonara.exe").is_file()
    assert not (box.root / ".bootstrap.lock").exists()


def test_the_wrapper_names_the_plugin_for_the_doctor(box, exes):
    # A slash command's Bash tool may lack CLAUDE_PLUGIN_ROOT: bin/sonara
    # sets it from its own folder, so the doctor can check the hooks.
    box.install(exes)
    del box.env["CLAUDE_PLUGIN_ROOT"]
    p = box.wrapper("doctor")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "[ OK ] hooks:" in p.stdout, p.stdout
