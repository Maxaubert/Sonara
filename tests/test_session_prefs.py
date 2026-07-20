"""Per-session prefs store: durable name/mute/voice map (#session-manager)."""
import json

from sonara.session_prefs import SessionPrefs


def test_defaults_for_unknown_session():
    p = SessionPrefs()
    assert p.name("s1") is None
    assert p.muted("s1") is False
    assert p.voice("s1") is None
    assert p.get("s1") == {}


def test_set_and_read_back():
    p = SessionPrefs()
    assert p.set("s1", "name", "build box")
    assert p.set("s1", "muted", True)
    assert p.set("s1", "voice", "af_heart")
    assert p.name("s1") == "build box"
    assert p.muted("s1") is True
    assert p.voice("s1") == "af_heart"


def test_falsy_name_and_voice_clear_the_key():
    p = SessionPrefs()
    p.set("s1", "name", "x")
    p.set("s1", "voice", "af_heart")
    p.set("s1", "name", "")
    p.set("s1", "voice", None)
    assert p.name("s1") is None
    assert p.voice("s1") is None


def test_unknown_key_and_bad_session_rejected():
    p = SessionPrefs()
    assert p.set("s1", "rate", 300) is False
    assert p.set(None, "name", "x") is False
    assert p.get("s1") == {}


def test_name_capped_at_60_chars():
    p = SessionPrefs()
    p.set("s1", "name", "x" * 200)
    assert len(p.name("s1")) == 60


def test_persist_roundtrip(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "alpha")
    p.set("s1", "muted", True)
    p2 = SessionPrefs(store_path=store)
    assert p2.name("s1") == "alpha"
    assert p2.muted("s1") is True


def test_forget_removes_and_persists(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "alpha")
    p.forget("s1")
    assert p.get("s1") == {}
    assert SessionPrefs(store_path=store).get("s1") == {}


def test_corrupt_store_is_a_silent_noop(tmp_path):
    store = tmp_path / "session_prefs.json"
    store.write_text("{not json", encoding="utf-8")
    p = SessionPrefs(store_path=store)
    assert p.get("s1") == {}
    assert p.set("s1", "name", "ok")           # still writable after corruption


def test_cap_evicts_oldest(tmp_path):
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store, store_cap=3)
    for i in range(5):
        p.set(f"s{i}", "name", f"n{i}")
    data = json.loads(store.read_text(encoding="utf-8"))
    assert len(data) == 3
    assert "s0" not in data and "s4" in data


def test_empty_entry_dropped_from_store(tmp_path):
    # clearing the last pref removes the whole entry (no {} litter)
    store = tmp_path / "session_prefs.json"
    p = SessionPrefs(store_path=store)
    p.set("s1", "name", "x")
    p.set("s1", "name", "")
    data = json.loads(store.read_text(encoding="utf-8"))
    assert "s1" not in data
