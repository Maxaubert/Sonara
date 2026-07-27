"""DigestStore: durable per-session last-digest text (#118). Mirrors the
sessions.py / session_prefs.py storage discipline."""
from sonara.digest_store import DigestStore, _TEXT_MAX


def test_set_get_forget_roundtrip():
    d = DigestStore()
    d.set("A", "alpha digest")
    assert d.get("A") == "alpha digest"
    d.set("A", "newer digest")
    assert d.get("A") == "newer digest"
    d.forget("A")
    assert d.get("A") is None


def test_items_lists_all_entries():
    d = DigestStore()
    d.set("A", "one")
    d.set("B", "two")
    assert dict(d.items()) == {"A": "one", "B": "two"}


def test_invalid_inputs_ignored():
    d = DigestStore()
    d.set("", "text")
    d.set(None, "text")
    d.set("A", "")
    d.set("A", None)
    assert d.items() == []
    d.forget("missing")           # no raise


def test_text_capped():
    d = DigestStore()
    d.set("A", "x" * (_TEXT_MAX + 100))
    assert len(d.get("A")) == _TEXT_MAX


def test_persists_and_reloads(tmp_path):
    p = tmp_path / "digests.json"
    d = DigestStore(store_path=p)
    d.set("A", "alpha digest")
    d.set("B", "beta digest")
    d2 = DigestStore(store_path=p)
    assert d2.get("A") == "alpha digest"
    assert d2.get("B") == "beta digest"
    d.forget("A")
    d3 = DigestStore(store_path=p)
    assert d3.get("A") is None and d3.get("B") == "beta digest"


def test_cap_keeps_newest_entries(tmp_path):
    p = tmp_path / "digests.json"
    d = DigestStore(store_path=p, store_cap=2)
    d.set("A", "one"); d.set("B", "two"); d.set("C", "three")
    d2 = DigestStore(store_path=p, store_cap=2)
    assert d2.get("A") is None                     # oldest evicted
    assert d2.get("B") == "two" and d2.get("C") == "three"


def test_reset_touches_move_to_newest(tmp_path):
    # Overwriting an existing session re-inserts it at the newest position, so
    # an active session is never the one the cap evicts.
    p = tmp_path / "digests.json"
    d = DigestStore(store_path=p, store_cap=2)
    d.set("A", "one"); d.set("B", "two")
    d.set("A", "one updated")                      # A becomes newest
    d.set("C", "three")                            # evicts B, not A
    d2 = DigestStore(store_path=p, store_cap=2)
    assert d2.get("A") == "one updated"
    assert d2.get("B") is None


def test_corrupt_or_missing_file_tolerated(tmp_path):
    p = tmp_path / "digests.json"
    p.write_text("{not json", encoding="utf-8")
    d = DigestStore(store_path=p)                  # no raise
    assert d.items() == []
    d.set("A", "fresh")
    assert DigestStore(store_path=p).get("A") == "fresh"
