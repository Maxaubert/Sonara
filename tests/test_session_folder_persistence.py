"""Regression: the session-change announcement must keep the folder name across a
daemon restart. Before the fix, a background session whose cwd was recorded only in
the (now-lost) daemon memory announced "Session changed: another session"."""
from sonara.sessions import SessionManager
from sonara.router import Router
from sonara.queue import SpeechItem


def _announce(folder, replay=False):
    return "Session changed: {0}.".format(folder)


def _switch_to_b(sm):
    r = Router(sm, minqueue=lambda: 1, announce_text=_announce)
    # After a restart, background session B only emits cwd-less output (prose/earcons).
    r.channel("A").append(SpeechItem(id=1, session="A", kind="prose", text="a", is_decision=False))
    r.channel("B").append(SpeechItem(id=2, session="B", kind="prose", text="b", is_decision=False))
    r.active = "A"
    r._last_active = "A"
    r.next_session()                      # cycle the active reader to B
    return r.next_item()                  # emits the queued hand-off announcement


def test_folder_name_survives_daemon_restart(tmp_path):
    p = tmp_path / "sessions.json"
    # Daemon run 1: A is foreground, B is a background session registered with its cwd.
    sm1 = SessionManager(store_path=p)
    sm1.set_foreground("A", cwd="/home/me/projA")
    sm1.register("B", cwd="/home/me/Documents/myrepo")
    # Daemon RESTART: a fresh manager loads the persisted folder map.
    sm2 = SessionManager(store_path=p)
    item = _switch_to_b(sm2)
    assert item.text == "Session changed: myrepo."      # NOT "another session"


def test_without_persistence_the_bug_reproduces():
    # Documents the pre-fix behavior: with no recorded folder, the generic fallback fires.
    sm = SessionManager()                 # fresh, no store; B never had a cwd
    sm.set_foreground("A", cwd="/home/me/projA")
    item = _switch_to_b(sm)
    assert item.text == "Session changed: another session."
