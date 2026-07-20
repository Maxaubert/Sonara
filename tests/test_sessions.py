import json

from sonara.sessions import SessionManager


def test_foreground_starts_none():
    sm = SessionManager()
    assert sm.foreground() is None


def test_set_and_get_foreground():
    sm = SessionManager()
    sm.set_foreground("s1")
    assert sm.foreground() == "s1"
    assert sm.is_foreground("s1") is True
    assert sm.is_foreground("s2") is False


def test_set_foreground_replaces_previous():
    sm = SessionManager()
    sm.set_foreground("s1")
    sm.set_foreground("s2")
    assert sm.foreground() == "s2"
    assert sm.is_foreground("s1") is False
    assert sm.is_foreground("s2") is True


def test_should_speak_true_only_for_foreground():
    sm = SessionManager()
    sm.register("s1")
    sm.register("s2")
    sm.set_foreground("s1")
    assert sm.should_speak("s1") is True
    assert sm.should_speak("s2") is False


def test_should_speak_false_when_no_foreground():
    sm = SessionManager()
    sm.register("s1")
    assert sm.should_speak("s1") is False


def test_register_and_unregister():
    sm = SessionManager()
    sm.register("s1")
    sm.set_foreground("s1")
    assert sm.is_foreground("s1") is True
    sm.unregister("s1")
    # unregistering the foreground session clears foreground
    assert sm.foreground() is None
    assert sm.should_speak("s1") is False


def test_unregister_non_foreground_keeps_foreground():
    sm = SessionManager()
    sm.register("s1")
    sm.register("s2")
    sm.set_foreground("s1")
    sm.unregister("s2")
    assert sm.foreground() == "s1"


def test_unregister_unknown_session_is_noop():
    sm = SessionManager()
    sm.set_foreground("s1")
    sm.unregister("ghost")
    assert sm.foreground() == "s1"


# --- cwd capture + folder name -------------------------------------------

def test_register_records_cwd_basename_posix():
    sm = SessionManager()
    sm.register("s1", cwd="/home/me/myapp")
    assert sm.folder("s1") == "myapp"


def test_set_foreground_records_cwd_basename_windows_path():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="C:\\Users\\me\\proj")
    assert sm.folder("s1") == "proj"      # portable: handles backslashes on any host


def test_folder_unknown_session_is_none():
    sm = SessionManager()
    assert sm.folder("nope") is None


def test_empty_cwd_does_not_clobber_known_folder():
    sm = SessionManager()
    sm.register("s1", cwd="/x/myapp")
    sm.set_foreground("s1", cwd="")       # later message with no cwd
    assert sm.folder("s1") == "myapp"     # keep the good name


# --- folder-map persistence (survives daemon restart) --------------------

def test_store_writes_folder_map_on_record(tmp_path):
    p = tmp_path / "sessions.json"
    sm = SessionManager(store_path=p)
    sm.register("s1", cwd="/home/me/myapp")
    assert json.loads(p.read_text(encoding="utf-8")).get("s1") == "myapp"


def test_store_reload_recovers_folder(tmp_path):
    p = tmp_path / "sessions.json"
    SessionManager(store_path=p).set_foreground("s1", cwd="C:\\Users\\me\\proj")
    sm2 = SessionManager(store_path=p)               # fresh manager == daemon restart
    assert sm2.folder("s1") == "proj"


def test_no_store_path_is_pure(tmp_path):
    p = tmp_path / "sessions.json"
    sm = SessionManager()                            # default: no persistence
    sm.register("s1", cwd="/x/myapp")
    assert not p.exists()                            # nothing written when no store
    assert sm.folder("s1") == "myapp"                # in-memory behavior unchanged


def test_none_folder_is_not_persisted(tmp_path):
    p = tmp_path / "sessions.json"
    sm = SessionManager(store_path=p)
    sm.register("s1", cwd=None)                       # unknown folder
    sm.register("s2", cwd="/x/known")
    data = json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}
    assert "s1" not in data                          # nothing useful to save
    assert data.get("s2") == "known"


def test_unregister_persists_removal(tmp_path):
    p = tmp_path / "sessions.json"
    sm = SessionManager(store_path=p)
    sm.register("s1", cwd="/x/myapp")
    sm.unregister("s1")
    data = json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}
    assert "s1" not in data


def test_store_caps_to_most_recent(tmp_path):
    p = tmp_path / "sessions.json"
    sm = SessionManager(store_path=p, store_cap=3)
    for i in range(5):
        sm.register("s{0}".format(i), cwd="/x/folder{0}".format(i))
    data = json.loads(p.read_text(encoding="utf-8"))
    assert set(data) == {"s2", "s3", "s4"}           # oldest two dropped from the file


def test_load_tolerates_missing_and_corrupt(tmp_path):
    assert SessionManager(store_path=tmp_path / "nope.json").folder("x") is None
    corrupt = tmp_path / "bad.json"
    corrupt.write_text("{ not json", encoding="utf-8")
    assert SessionManager(store_path=corrupt).folder("x") is None


def test_reload_does_not_override_live_record(tmp_path):
    p = tmp_path / "sessions.json"
    SessionManager(store_path=p).register("s1", cwd="/x/old")
    sm2 = SessionManager(store_path=p)
    sm2.register("s1", cwd="/x/new")                 # folder moved this run
    assert sm2.folder("s1") == "new"
    assert json.loads(p.read_text(encoding="utf-8"))["s1"] == "new"



def test_touch_and_last_seen():
    import time
    m = SessionManager()
    assert m.last_seen("s1") is None
    before = time.time()
    m.touch("s1")
    seen = m.last_seen("s1")
    assert seen is not None and before <= seen <= time.time()


def test_register_touches():
    m = SessionManager()
    m.register("s1", cwd="/x/proj")
    assert m.last_seen("s1") is not None


def test_unregister_clears_last_seen():
    m = SessionManager()
    m.touch("s1")
    m.unregister("s1")
    assert m.last_seen("s1") is None


def test_store_load_does_not_touch(tmp_path):
    # a session restored from disk must look INACTIVE until it sends real
    # traffic: recency is the liveness signal for the Sessions tab
    store = tmp_path / "sessions.json"
    m1 = SessionManager(store_path=store)
    m1.register("s1", cwd="/x/proj")
    m2 = SessionManager(store_path=store)
    assert m2.folder("s1") == "proj"
    assert m2.last_seen("s1") is None
