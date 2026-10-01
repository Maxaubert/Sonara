"""Per-session state registry (#141): every feature registers the state it
keeps per session, and ending or forgetting a session clears all of it in one
call (core.SessionRegistry.forget_session) instead of a hand-written list of
pops that a new feature could forget to extend."""
import pytest

from sonara.daemon import core
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def test_forget_session_clears_registered_dicts_and_sets_for_that_session_only():
    reg = core.SessionRegistry()
    by_session = {"a": 1, "b": 2}
    flagged = {"a", "b"}
    reg.register("by_session", by_session)
    reg.register("flagged", flagged)
    reg.forget_session("a")
    assert by_session == {"b": 2}
    assert flagged == {"b"}


def test_forget_session_runs_hooks_in_registration_order():
    reg = core.SessionRegistry()
    calls = []
    reg.register_hook("first", lambda sid: calls.append(("first", sid)))
    reg.register("store", {})
    reg.register_hook("second", lambda sid: calls.append(("second", sid)))
    reg.forget_session("s1")
    assert calls == [("first", "s1"), ("second", "s1")]
    assert reg.names() == ["first", "store", "second"]


def test_forget_session_on_an_unknown_session_is_a_no_op():
    reg = core.SessionRegistry()
    d, s = {"a": 1}, {"a"}
    reg.register("d", d)
    reg.register("s", s)
    reg.forget_session("zzz")
    assert d == {"a": 1} and s == {"a"}


def test_register_refuses_duplicate_names_and_non_containers():
    reg = core.SessionRegistry()
    reg.register("x", {})
    with pytest.raises(ValueError):
        reg.register("x", set())
    with pytest.raises(ValueError):
        reg.register_hook("x", lambda sid: None)
    with pytest.raises(TypeError):
        reg.register("y", [])
    with pytest.raises(TypeError):
        reg.register_hook("z", "not callable")


def test_the_daemon_registers_every_feature_s_per_session_state():
    daemon, *_ = make_daemon()
    # An exact set, not a subset: adding or dropping a per-session store
    # must fail here so the change gets a deliberate review.
    names = set(daemon._session_state.names())
    assert names == {"await_choice", "warned_immediate", "assemblers",
                     "last_digest_text", "pending_heard", "history",
                     "setup_guide", "summary", "digest_store"}


@pytest.mark.parametrize("mtype", [MsgType.SESSION_END, MsgType.FORGET_SESSION])
def test_ending_a_session_clears_every_registered_store(mtype):
    daemon, *_ = make_daemon(foreground="fg")
    stores = daemon._session_state.stores()
    assert stores, "the daemon registers per-session stores"
    for _name, store in stores:
        if isinstance(store, dict):
            store["gone"] = object()
            store["kept"] = object()
        else:
            store.add("gone")
            store.add("kept")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": mtype,
                           "session": "gone"})
    for name, store in stores:
        assert "gone" not in store, name
        assert "kept" in store, name
