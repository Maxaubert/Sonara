"""Core: authentication and the hello handshake (spec 4, 4.1)."""
from __future__ import annotations


def test_first_tcp_message_must_be_hello(rt):
    c = rt.tcp(hello=False)
    r = c.request({"type": "speak", "text": "Hi.", "id": 1})
    assert r["ok"] is False
    assert r["error"]["code"] == "E_AUTH"
    assert r["id"] == 1
    assert c.closed()


def test_wrong_token_is_e_auth_and_closes(rt):
    c = rt.tcp(hello=False)
    r = c.hello("not-the-token")
    assert r["error"]["code"] == "E_AUTH"
    assert c.closed()


def test_garbage_before_hello_is_e_auth(rt):
    c = rt.tcp(hello=False)
    c.send_raw(b"this is not json\n")
    r = c.reply()
    assert r["error"]["code"] == "E_AUTH"
    assert c.closed()


def test_http_needs_the_bearer_token(rt):
    status, body = rt.post("get", {"key": "volume"}, token="")
    assert status == 401
    assert body["error"]["code"] == "E_AUTH"
    status, body = rt.post("get", {"key": "volume"}, token="wrong")
    assert status == 401
    status, body = rt.post("get", {"key": "volume"})
    assert status == 200
    assert body == {"ok": True, "key": "volume", "value": 100}


def test_sse_needs_the_bearer_token(rt):
    import urllib.error

    try:
        rt.sse(token="wrong")
    except urllib.error.HTTPError as e:
        assert e.code == 401
    else:
        raise AssertionError("SSE without the token was accepted")


def test_hello_reply(rt):
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, id="h", protocol={"major": 1, "minor": 0})
    assert r["ok"] is True
    assert r["id"] == "h"
    assert r["version"] == rt.info["version"]
    assert r["protocol"] == {"major": 1, "minor": 0}
    assert "core" in r["capabilities"]
    assert set(r["capabilities"]) == set(rt.info["capabilities"])
    assert r["extensions"] == []


def test_hello_require_unmet_is_e_unsupported(rt):
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, require=["core", "no-such-extension"])
    assert r["error"]["code"] == "E_UNSUPPORTED"
    assert "no-such-extension" in r["error"]["message"]
    # Not authenticated: the client may retry hello.
    r = c.hello(rt.token, require=["core"])
    assert r["ok"] is True


def test_requested_extensions_are_listed_unavailable(rt):
    # Host-agnostic: whatever this host offers is enabled, the rest is
    # listed unavailable; an extension it does not know is never an error.
    c = rt.tcp(hello=False)
    asked = ["channels", "no-such-extension"]
    r = c.hello(rt.token, extensions=asked)
    assert r["ok"] is True
    assert "no-such-extension" in r["unavailable"]
    assert set(r["unavailable"]) <= set(asked)
    assert not set(r["unavailable"]) & set(r["extensions"])
    assert set(asked) <= set(r["unavailable"]) | set(r["extensions"])


def test_another_protocol_major_is_e_incompatible(rt):
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, protocol={"major": 2, "minor": 0})
    assert r["error"]["code"] == "E_INCOMPATIBLE"


def test_unknown_hello_fields_are_ignored(rt):
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, future={"x": 1}, color="blue")
    assert r["ok"] is True


def test_hello_over_http(rt):
    status, r = rt.post("hello", {"client": {"name": "curl"}})
    assert status == 200
    assert r["protocol"]["major"] == 1
