"""Table dispatch (#141): handle_message looks the handler up by message type
in a table each feature module fills. Every protocol type has exactly one
owner, and unknown or malformed types get no reply, as the old if-chain
did."""
import pytest

from sonara.daemon import core
from sonara.protocol import MsgType
from tests.daemon_helpers import make_daemon


def _all_types():
    return {v for k, v in vars(MsgType).items() if k.isupper()}


def test_every_protocol_message_type_has_a_handler():
    daemon, *_ = make_daemon()
    assert set(daemon._handlers) == _all_types()


@pytest.mark.parametrize("mtype", ["no_such_type", None, 42, ["prose"], {"a": 1}])
def test_unknown_or_malformed_types_return_none(mtype):
    daemon, *_ = make_daemon()
    assert daemon.handle_message({"v": 1, "type": mtype, "session": "fg"}) is None


def test_a_message_type_cannot_get_two_handlers():
    table = {}
    core.add_handlers(table, {MsgType.PING: lambda m: 1})
    with pytest.raises(ValueError):
        core.add_handlers(table, {MsgType.PING: lambda m: 2})
    assert table[MsgType.PING]({}) == 1


def test_ping_and_status_still_reply():
    daemon, *_ = make_daemon()
    assert daemon.handle_message({"v": 1, "type": MsgType.PING}) == {"ok": True}
    status = daemon.handle_message({"v": 1, "type": MsgType.STATUS})
    assert status["foreground"] == "fg"
