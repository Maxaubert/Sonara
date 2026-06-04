import importlib
from pathlib import Path


def _fresh_paths(monkeypatch, home):
    """Reload echo.paths so the module-level Path.home() constants pick up the patched HOME."""
    monkeypatch.setenv("HOME", str(home))
    import echo.paths as paths
    return importlib.reload(paths)


def test_constants_end_with_expected_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.ECHO_DIR.name == ".echo"
    assert paths.CONFIG_PATH.name == "config.json"
    assert paths.SOCKET_PATH.name == "speechd.sock"
    assert paths.LOG_PATH.name == "speechd.log"


def test_paths_are_nested_under_echo_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.CONFIG_PATH.parent == paths.ECHO_DIR
    assert paths.SOCKET_PATH.parent == paths.ECHO_DIR
    assert paths.LOG_PATH.parent == paths.ECHO_DIR


def test_echo_dir_is_under_home(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.ECHO_DIR == Path(tmp_path) / ".echo"


def test_ensure_echo_dir_creates_directory(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert not paths.ECHO_DIR.exists()
    paths.ensure_echo_dir()
    assert paths.ECHO_DIR.is_dir()


def test_ensure_echo_dir_is_idempotent(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    paths.ensure_echo_dir()
    paths.ensure_echo_dir()  # must not raise on an existing dir
    assert paths.ECHO_DIR.is_dir()
