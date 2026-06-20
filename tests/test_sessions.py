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

