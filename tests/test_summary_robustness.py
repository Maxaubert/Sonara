"""Summary pipeline robustness (#138): a hung digest worker, a summarizer that
ignores its timeout, a decision overwritten inside the settle window, a settle
fire that raises, and the daemon's working directory. Each one could stop the
latest turn from ever being spoken."""
import os
import subprocess
import sys
import time

import pytest

import sonara.daemon as daemon_module
from sonara import summarizer
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon

_PAD = "This filler sentence carries the turn well past the digest threshold. "


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _prose(session, text):
    return _msg(MsgType.PROSE, session, delta=text, index=0, final=True)


def _choice(question):
    return {"questions": [{"question": question, "options": [{"label": "Yes"}]}]}


def _daemon(monkeypatch, foreground="user"):
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground=foreground)
    config["summary_mode"] = True
    monkeypatch.setattr(daemon, "_settle_schedule", lambda session, gen: None)
    for s in ("user", "a", "b", "fg"):
        sessions.register(s, cwd="/w/" + s)
    return daemon, speaker


def _capture_spawn(daemon, monkeypatch):
    calls = []

    def fake(session, gen, text, token=0, leadin=False, seq=None):
        calls.append({"session": session, "gen": gen, "text": text,
                      "token": token, "leadin": leadin, "seq": seq})

    monkeypatch.setattr(daemon, "_start_summary_thread", fake)
    return calls


def _capture_watchdogs(daemon, monkeypatch):
    armed = []
    monkeypatch.setattr(daemon, "_schedule_digest_watchdog",
                        lambda seq, apply: armed.append((seq, apply)))
    return armed


def _fire_settle(daemon, session):
    daemon._settle_fire(session, daemon._settle_gen.get(session))


def _finish_turn(daemon, session, label):
    daemon.handle_message(_prose(session, "Report {0}. ".format(label) + _PAD * 6))
    daemon.handle_message(_msg(MsgType.EARCON, session, kind="turn_done"))
    _fire_settle(daemon, session)


def _drain(daemon, speaker, n=30):
    daemon._poll_interval = 0.01
    for _ in range(n):
        daemon._speak_loop_once()
    return list(speaker.spoken)


def _run_worker(daemon, call, digest):
    daemon._summarize_fn = lambda text, **kw: digest
    daemon._summary_worker(call["session"], call["gen"], call["text"],
                           call["token"], call["leadin"], call["seq"])


# --- M1: a hung digest worker cannot park later digests forever --------------

def test_hung_digest_slot_is_landed_by_the_watchdog(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    armed = _capture_watchdogs(daemon, monkeypatch)
    _finish_turn(daemon, "a", "alpha")           # seq 0: its worker hangs
    _finish_turn(daemon, "b", "bravo")           # seq 1
    assert [seq for seq, _ in armed] == [0, 1]   # one watchdog per dispatched seq
    _run_worker(daemon, calls[1], "Recap bravo")
    assert not [t for t in _drain(daemon, speaker, n=6) if "bravo" in t.lower()]
    daemon._digest_watchdog_fire(*armed[0])      # a's worker never came back
    heard = _drain(daemon, speaker)
    # The hung turn still speaks (raw fallback: never skip the last message)
    # and the digest parked behind it is released, in dispatch order.
    alpha = [i for i, t in enumerate(heard) if "Report alpha" in t]
    bravo = [i for i, t in enumerate(heard) if t == "Recap bravo"]
    assert alpha and bravo and alpha[0] < bravo[0]


def test_late_worker_after_the_watchdog_is_ignored(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    armed = _capture_watchdogs(daemon, monkeypatch)
    _finish_turn(daemon, "a", "alpha")
    daemon._digest_watchdog_fire(*armed[0])
    _drain(daemon, speaker)
    _run_worker(daemon, calls[0], "Recap alpha")  # the hung worker finally returns
    heard = _drain(daemon, speaker)
    assert "Recap alpha" not in heard
    assert sum("Report alpha" in t for t in heard) == 1
    assert daemon._digest_parked == {}


def test_watchdog_is_a_noop_once_the_worker_landed(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    armed = _capture_watchdogs(daemon, monkeypatch)
    _finish_turn(daemon, "a", "alpha")
    _run_worker(daemon, calls[0], "Recap alpha")
    daemon._digest_watchdog_fire(*armed[0])
    heard = _drain(daemon, speaker)
    assert heard.count("Recap alpha") == 1
    assert not any("Report alpha" in t for t in heard)


def test_watchdog_waits_twice_the_summary_timeout(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    daemon.config["summary_timeout"] = 45
    timers = []

    class _Timer:
        def __init__(self, interval, fn, args=()):
            timers.append(interval)
            self.daemon = False

        def start(self):
            pass

        def cancel(self):
            pass

    monkeypatch.setattr(daemon_module.threading, "Timer", _Timer)
    daemon._schedule_digest_watchdog(0, None)
    assert timers == [90.0]


# --- L-settle-fire: an exception in the settle fire still lands its seq ------

def test_settle_fire_failure_still_lands_its_slot(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    _capture_watchdogs(daemon, monkeypatch)

    def boom(*a, **k):
        raise RuntimeError("cannot start thread")

    monkeypatch.setattr(daemon, "_start_summary_thread", boom)
    _finish_turn(daemon, "a", "alpha")           # must not raise
    assert daemon._digest_seq_serve == daemon._digest_seq_next
    assert daemon._digest_parked == {}
    assert not daemon._inflight_digests.get("a")
    heard = _drain(daemon, speaker)
    assert any("Report alpha" in t for t in heard)   # the turn still speaks


def test_settle_fire_failure_never_loses_the_question(monkeypatch):
    daemon, speaker = _daemon(monkeypatch, foreground="fg")
    _capture_watchdogs(daemon, monkeypatch)
    monkeypatch.setattr(daemon, "_maybe_summarize",
                        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("x")))
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", **_choice("Deploy now?")))
    _fire_settle(daemon, "fg")                   # must not raise
    ch = daemon.router.channel("fg")
    assert any(it.is_decision and "Deploy now?" in it.text for it in ch.items)


def test_a_raising_release_does_not_strand_later_slots(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    ran = []
    s0, s1, s2 = (daemon._alloc_digest_seq() for _ in range(3))
    daemon._land_digest(s2, lambda: ran.append(2))
    daemon._land_digest(s1, lambda: (_ for _ in ()).throw(RuntimeError("x")))
    daemon._land_digest(s0, lambda: ran.append(0))
    assert ran == [0, 2]
    assert daemon._digest_parked == {}


# --- L-settle-pop: teardown keeps the settle generation monotonic -----------

def test_stale_settle_fire_after_teardown_and_rearm_is_a_noop(monkeypatch):
    daemon, speaker = _daemon(monkeypatch)
    calls = _capture_spawn(daemon, monkeypatch)
    _capture_watchdogs(daemon, monkeypatch)
    daemon.handle_message(_prose("a", "Report alpha. " + _PAD * 6))
    daemon.handle_message(_msg(MsgType.EARCON, "a", kind="turn_done"))
    stale = daemon._settle_gen["a"]              # a fire blocked on the lock
    daemon.handle_message(_msg(MsgType.SESSION_END, "a"))
    daemon.handle_message(_prose("a", "Report again. " + _PAD * 6))
    daemon.handle_message(_msg(MsgType.EARCON, "a", kind="turn_done"))
    daemon._settle_fire("a", stale)
    assert calls == []                           # the stale fire did nothing
    _fire_settle(daemon, "a")
    assert len(calls) == 1


# --- F7: two decisions inside one settle window ------------------------------

def test_second_decision_in_settle_window_does_not_drop_the_first(monkeypatch):
    daemon, speaker = _daemon(monkeypatch, foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    monkeypatch.setattr(daemon, "_schedule_hold_release", lambda *a: None)
    daemon.handle_message(_prose("fg", "Let me look at this first."))
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", **_choice("First question?")))
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Second plan."))
    _fire_settle(daemon, "fg")
    assert len(calls) == 1 and calls[0]["leadin"]
    _run_worker(daemon, calls[0], "Context first.")
    heard = _drain(daemon, speaker)
    order = [next(i for i, t in enumerate(heard) if key in t)
             for key in ("Context first.", "First question?", "Second plan.")]
    assert order == sorted(order)
    assert daemon._pending_heard == {}


def test_flush_drops_pending_decision_bookkeeping(monkeypatch):
    daemon, speaker = _daemon(monkeypatch, foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", **_choice("Deploy now?")))
    assert daemon._pending_heard
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert daemon._pending_heard == {}


def test_session_end_drops_held_decision_bookkeeping(monkeypatch):
    daemon, speaker = _daemon(monkeypatch, foreground="fg")
    _capture_spawn(daemon, monkeypatch)
    monkeypatch.setattr(daemon, "_schedule_hold_release", lambda *a: None)
    daemon.handle_message(_prose("fg", "Context. " * 40))
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", **_choice("Deploy now?")))
    _fire_settle(daemon, "fg")
    assert daemon._held_decision.get("fg") is not None
    daemon.handle_message(_msg(MsgType.SESSION_END, "fg"))
    assert daemon._pending_heard == {}


# --- F5: the summarizer timeout is enforced for .cmd engines -----------------

class _HangingProc:
    """A cmd.exe whose grandchild holds the pipes: communicate() only returns
    via its timeout, and a second, unbounded communicate() would block."""
    pid = 4242
    returncode = None

    def __init__(self):
        self.calls = []

    def communicate(self, input=None, timeout=None):
        self.calls.append(timeout)
        if timeout is None:
            raise AssertionError("unbounded communicate() would block forever")
        raise subprocess.TimeoutExpired("codex", timeout)

    def kill(self):
        pass

    def wait(self, timeout=None):
        return 1


def test_timeout_kills_the_process_tree_without_blocking(monkeypatch, tmp_path):
    proc = _HangingProc()
    killed = []
    monkeypatch.setattr(summarizer.subprocess, "Popen", lambda *a, **k: proc)
    monkeypatch.setattr(summarizer, "_kill_tree", lambda p: killed.append(p.pid))
    monkeypatch.setattr(summarizer, "_resolve_command", lambda name: "codex.cmd")
    with pytest.raises(subprocess.TimeoutExpired):
        summarizer._default_runner(["codex", "exec"], "text", 3)
    assert killed == [4242]
    assert None not in proc.calls


@pytest.mark.skipif(os.name != "nt", reason="a .cmd shim is Windows-only")
def test_real_cmd_shim_with_a_grandchild_times_out(tmp_path):
    shim = tmp_path / "slow.cmd"
    shim.write_text('@"{0}" -c "import time; time.sleep(60)"\r\n'.format(
        sys.executable))
    t0 = time.monotonic()
    out = summarizer.summarize("hello", model="m", command=str(shim), timeout=2)
    assert out is None
    assert time.monotonic() - t0 < 20


# --- H1: the daemon's cwd and the summarizer engine lookup -------------------

def test_launch_spec_runs_the_daemon_in_the_sonara_dir():
    from sonara import paths
    from sonara.platform.windows import supervisor_loop as sl
    argv, kwargs = sl.launch_spec(r"C:\Python311\pythonw.exe")
    try:
        assert kwargs["cwd"] == str(paths.SONARA_DIR)
    finally:
        kwargs["stderr"].close()


def test_lazy_start_spawns_in_the_sonara_dir(monkeypatch):
    from sonara import paths
    from sonara.platform.windows import supervisor as sup_mod
    monkeypatch.setattr(daemon_module, "socket_connectable", lambda: False)
    monkeypatch.setattr(sup_mod, "daemon_pythonw", lambda: r"C:\Python311\pythonw.exe")
    spawned = []
    monkeypatch.setattr(daemon_module.subprocess, "Popen",
                        lambda argv, **k: spawned.append(k))

    class _Plat:
        supervisor = sup_mod.WinSupervisorBackend()

    monkeypatch.setattr("sonara.platform.get_platform", lambda: _Plat())
    daemon_module.ensure_running()
    try:
        assert spawned[0]["cwd"] == str(paths.SONARA_DIR)
    finally:
        spawned[0]["stderr"].close()


def _plant(directory, names):
    directory.mkdir(parents=True, exist_ok=True)
    for name in names:
        p = directory / name
        p.write_text("")
        p.chmod(0o755)


def test_engine_planted_in_the_cwd_is_not_picked(monkeypatch, tmp_path):
    cwd = tmp_path / "project"
    _plant(cwd, ["claude", "claude.exe", "claude.cmd"])
    empty = tmp_path / "bin"
    empty.mkdir()
    monkeypatch.chdir(cwd)
    monkeypatch.setenv("PATH", str(empty))
    monkeypatch.delenv("NoDefaultCurrentDirectoryInExePath", raising=False)
    assert summarizer._resolve_command("claude") is None


def test_engine_on_path_is_found(monkeypatch, tmp_path):
    bindir = tmp_path / "bin"
    _plant(bindir, ["claude.cmd" if os.name == "nt" else "claude"])
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PATH", str(bindir))
    found = summarizer._resolve_command("claude")
    assert found is not None
    assert os.path.dirname(found) == str(bindir)


def test_missing_engine_is_logged_and_never_spawned(monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("PATH", str(tmp_path / "nowhere"))
    monkeypatch.setattr(
        summarizer.subprocess, "Popen",
        lambda *a, **k: (_ for _ in ()).throw(AssertionError("spawned")))
    logs = []
    out = summarizer.summarize("hello", model="m", command="claude",
                               debug_log=logs.append)
    assert out is None
    assert any("not found" in line for line in logs)
