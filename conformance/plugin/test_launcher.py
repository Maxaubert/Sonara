"""bin/sonara-hook-launch and bin/sonara-bootstrap.ps1 (#202): the plugin's
hooks run the runtime's sonara-hook.exe, and install the runtime from the
GitHub release first when it is missing, without ever blocking or failing
a Claude session."""
from __future__ import annotations

import json
import os
import subprocess
import time

import plugin_harness as ph

QUICK = ph.QUICK
NO_START = {"SONARA_BOOTSTRAP_START": "0"}
# A release newer than the plugin's (bin/runtime-version), as a newer plugin
# installs it while sessions of this one keep running.
NEWER = "99.0.0"


def serve_good_release(box, releases, exes):
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(data)})
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    return r


def test_a_missing_runtime_is_installed_in_the_background(box, releases, exes):
    # The downloads wait until the hook has returned: the order proves the
    # hook never waits for the bootstrap, whatever the runner's speed (#249).
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(data)}, hold=True)
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    code, took = box.launch("SessionStart", {"session_id": "s1"}, NO_START)
    assert code == 0
    assert not box.dest.exists(), "the hook returned before the install"
    assert took < QUICK, f"the hook took {took:.2f} s"
    r.open_gate()
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


def run_bootstrap(box, *args):
    """bin/sonara-bootstrap.ps1 in the foreground, as bin/sonara runs it."""
    env = {k: v for k, v in box.env.items() if k.upper() != "PSMODULEPATH"}
    return subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
         "-File", str(ph.BIN / "sonara-bootstrap.ps1"), "-Version", ph.VERSION, *args],
        capture_output=True, text=True, env=env, timeout=180,
        creationflags=0x08000000)


def test_an_older_plugin_uses_a_newer_installed_runtime(box, releases, exes, tmp_path):
    # Sessions started before a plugin update keep the old plugin folder:
    # their hooks must use the newer runtime, not download their own
    # release again (which would replace the newer one, and so on).
    r = serve_good_release(box, releases, exes)
    newer = box.install(exes, NEWER)
    capture = tmp_path / "capture"
    code, took = box.launch("MessageDisplay", {"session_id": "s1"},
                            {"SONARA_CAPTURE": str(capture), "SONARA_NO_START": "1"})
    assert code == 0
    assert took < QUICK
    assert list(capture.glob("MessageDisplay-*.json")), "the newer runtime got the event"
    assert r.hits == {}, "nothing downloaded"
    assert not box.dest.exists()
    assert not (box.root / ".bootstrap.lock").exists()
    assert newer.is_dir()


def test_an_older_runtime_does_not_serve_the_plugin(box, releases, exes):
    serve_good_release(box, releases, exes)
    box.install(exes, "0.0.1")
    code, _ = box.launch("SessionStart", {}, NO_START)
    assert code == 0
    box.wait_for(lambda: (box.dest / "sonara-hook.exe").is_file(), what="the install")
    box.wait_bootstrap()


def test_the_wrapper_of_an_older_plugin_uses_a_newer_runtime(box, releases, exes):
    r = serve_good_release(box, releases, exes)
    newer = box.install(exes, NEWER)
    p = box.wrapper("version")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "Installing" not in p.stdout
    assert r.hits == {}
    assert not box.dest.exists()
    assert newer.is_dir()


def test_the_bootstrap_removes_older_runtimes_only(box, releases, exes):
    serve_good_release(box, releases, exes)
    old = box.install(exes, "0.0.1")
    newer = box.install(exes, NEWER)
    box.env.update(NO_START)
    p = run_bootstrap(box)
    assert p.returncode == 0, p.stdout + p.stderr
    assert (box.dest / "sonara-hook.exe").is_file()
    assert not old.exists()
    assert newer.is_dir(), "a newer plugin's runtime stays"


def test_a_bootstrap_with_nostart_keeps_the_older_runtime_that_still_runs(box, releases, exes):
    # bin/sonara installs with -NoStart: nothing replaced the runtime that
    # runs from the older folder, so the folder stays until `sonara start`.
    serve_good_release(box, releases, exes)
    old = box.install(exes, "0.0.1")
    p = run_bootstrap(box, "-NoStart")
    assert p.returncode == 0, p.stdout + p.stderr
    assert (box.dest / "sonara-hook.exe").is_file()
    assert (old / "sonara-hook.exe").is_file(), "the older runtime is left whole"


def test_a_bootstrap_keeps_an_install_that_appeared_during_its_download(box, releases, exes):
    # Two bootstraps (a lock taken over twice): the first one installs and
    # starts the runtime while the second still downloads. The second must
    # not delete that runtime.
    data = ph.fake_zip(exes)
    r = releases({ph.ZIP_NAME: data, "SHA256SUMS": ph.sums_for(data)}, hold=True)
    box.env["SONARA_RELEASE_BASE_URL"] = r.url
    env = {k: v for k, v in box.env.items() if k.upper() != "PSMODULEPATH"}
    env.update(NO_START)
    p = subprocess.Popen(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
         "-File", str(ph.BIN / "sonara-bootstrap.ps1"), "-Version", ph.VERSION],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, creationflags=0x08000000)
    box.wait_for(lambda: r.hits.get("SHA256SUMS"), what="the first download")
    box.install(exes)
    marker = box.dest / "in-use.txt"
    marker.write_bytes(b"")
    r.open_gate()
    out, err = p.communicate(timeout=180)
    assert p.returncode == 0, (out + err).decode(errors="replace")
    assert marker.is_file(), "the install that appeared was not deleted"
    assert (box.dest / "sonara-hook.exe").is_file()
    assert not [d for d in box.root.iterdir() if d.name.startswith(".")], "no leftovers"


def test_a_stale_lock_is_not_taken_over_while_another_hook_takes_it(box, releases, exes):
    # Taking over a stale lock goes through a second lock folder, so two
    # hooks that both saw it stale never both start a bootstrap.
    r = serve_good_release(box, releases, exes)
    lock = box.root / ".bootstrap.lock"
    lock.mkdir(parents=True)
    old = time.time() - 20 * 60
    os.utime(lock, (old, old))
    (box.root / ".bootstrap.lock.takeover").mkdir()
    code, took = box.launch("SessionStart", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    time.sleep(1.0)
    assert r.hits == {}, "no second bootstrap"
    (box.root / ".bootstrap.lock.takeover").rmdir()
    code, _ = box.launch("SessionStart", {}, NO_START)
    assert code == 0
    box.wait_for(lambda: (box.dest / "sonara-hook.exe").is_file(), what="the install")
    box.wait_bootstrap()
    assert not [d for d in box.root.iterdir() if d.name.startswith(".")], "no leftovers"


def test_a_home_with_forward_slashes_is_still_stopped(box, releases, exes):
    r = serve_good_release(box, releases, exes)
    box.home.mkdir(parents=True)
    (box.home / "stopped").write_bytes(b"")
    box.env["SONARA_HOME"] = str(box.home).replace("\\", "/")
    code, took = box.launch("SessionStart", {}, NO_START)
    assert (code, took < QUICK) == (0, True)
    time.sleep(1.0)
    assert r.hits == {}
    assert not box.dest.exists()


def test_a_localappdata_with_forward_slashes_finds_the_runtime(box, releases, exes, tmp_path):
    r = serve_good_release(box, releases, exes)
    box.install(exes)
    box.env["LOCALAPPDATA"] = str(box.lad).replace("\\", "/")
    capture = tmp_path / "capture"
    code, _ = box.launch("MessageDisplay", {"session_id": "s1"},
                         {"SONARA_CAPTURE": str(capture), "SONARA_NO_START": "1"})
    assert code == 0
    assert list(capture.glob("MessageDisplay-*.json")), "the installed runtime got the event"
    assert r.hits == {}, "nothing downloaded"
    p = box.wrapper("version")
    assert p.returncode == 0, p.stdout + p.stderr
    assert "Installing" not in p.stdout


def test_the_wrapper_takes_over_a_stale_lock_only(box, releases, exes):
    # The same rule as the hook launcher: a lock older than 15 minutes
    # belongs to a bootstrap that was killed.
    serve_good_release(box, releases, exes)
    lock = box.root / ".bootstrap.lock"
    lock.mkdir(parents=True)
    old = time.time() - 20 * 60
    os.utime(lock, (old, old))
    p = box.wrapper("version", timeout=60)
    assert p.returncode == 0, p.stdout + p.stderr
    assert (box.dest / "sonara.exe").is_file()
    assert not lock.exists()
