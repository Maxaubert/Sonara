import importlib
import os
from pathlib import Path
from unittest import mock


def _fresh_paths(monkeypatch, home):
    """Reload sonara.paths so the module-level Path.home() constants pick up the patched HOME."""
    monkeypatch.setenv("HOME", str(home))
    import sonara.paths as paths
    return importlib.reload(paths)


def test_constants_end_with_expected_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.SONARA_DIR.name == ".sonara"
    assert paths.CONFIG_PATH.name == "config.json"
    assert paths.LOCK_PATH.name == "daemon.lock"
    assert paths.LOG_PATH.name == "speechd.log"


def test_paths_are_nested_under_sonara_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.CONFIG_PATH.parent == paths.SONARA_DIR
    assert paths.LOCK_PATH.parent == paths.SONARA_DIR
    assert paths.LOG_PATH.parent == paths.SONARA_DIR


def test_phase2_path_constants_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.name == "keymap.json"
    assert paths.HOTKEYD_RESOLVED_PATH.name == "hotkeyd.resolved.json"
    assert paths.HOTKEYD_BIN_PATH.name == "sonara-hotkeyd"


def test_phase2_paths_nested_under_sonara_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.parent == paths.SONARA_DIR
    assert paths.HOTKEYD_RESOLVED_PATH.parent == paths.SONARA_DIR
    assert paths.HOTKEYD_BIN_PATH.parent == paths.SONARA_DIR


def test_sonara_dir_is_under_home(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.SONARA_DIR == Path(tmp_path) / ".sonara"


def test_ensure_sonara_dir_creates_directory(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert not paths.SONARA_DIR.exists()
    paths.ensure_sonara_dir()
    assert paths.SONARA_DIR.is_dir()


def test_ensure_sonara_dir_is_idempotent(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    paths.ensure_sonara_dir()
    paths.ensure_sonara_dir()  # must not raise on an existing dir
    assert paths.SONARA_DIR.is_dir()


# ---------------------------------------------------------------------------
# socket_connectable
# ---------------------------------------------------------------------------

def test_socket_connectable_returns_true_when_connect_succeeds():
    import sonara.paths as paths
    with mock.patch("sonara.platform.transport.connectable", return_value=True) as mocked:
        assert paths.socket_connectable() is True
        mocked.assert_called_once_with(paths.LOCK_PATH)


def test_socket_connectable_returns_false_on_oserror():
    import sonara.paths as paths
    with mock.patch("sonara.platform.transport.connectable", return_value=False) as mocked:
        assert paths.socket_connectable() is False
        mocked.assert_called_once_with(paths.LOCK_PATH)


# ---------------------------------------------------------------------------
# repo_root
# ---------------------------------------------------------------------------

def test_repo_root_returns_string():
    import sonara.paths as paths
    root = paths.repo_root()
    assert isinstance(root, str)


def test_repo_root_is_absolute():
    import sonara.paths as paths
    root = paths.repo_root()
    assert os.path.isabs(root)


def test_repo_root_contains_src_subdir():
    """The canonical repo root must have a src/ subdirectory."""
    import sonara.paths as paths
    root = paths.repo_root()
    assert os.path.isdir(os.path.join(root, "src")), (
        f"Expected a src/ directory under repo_root {root!r}"
    )


def test_repo_root_derivation_matches_file_location():
    """repo_root() must be two directory levels above paths.py."""
    import sonara.paths as paths
    paths_file = os.path.abspath(paths.__file__)
    expected = os.path.dirname(os.path.dirname(os.path.dirname(paths_file)))
    assert paths.repo_root() == expected


def test_install_record_path_lives_under_sonara_dir():
    from sonara import paths
    assert paths.INSTALL_RECORD_PATH == paths.SONARA_DIR / "install.json"


def test_app_dir_lives_under_sonara_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.APP_DIR == paths.SONARA_DIR / "app"
    assert paths.APP_DIR.name == "app"
    assert paths.APP_DIR.parent == paths.SONARA_DIR
