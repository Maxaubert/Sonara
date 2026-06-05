import importlib
import os
import socket
from pathlib import Path
from unittest import mock


def _fresh_paths(monkeypatch, home):
    """Reload sonari.paths so the module-level Path.home() constants pick up the patched HOME."""
    monkeypatch.setenv("HOME", str(home))
    import sonari.paths as paths
    return importlib.reload(paths)


def test_constants_end_with_expected_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.SONARI_DIR.name == ".sonari"
    assert paths.CONFIG_PATH.name == "config.json"
    assert paths.SOCKET_PATH.name == "speechd.sock"
    assert paths.LOG_PATH.name == "speechd.log"


def test_paths_are_nested_under_sonari_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.CONFIG_PATH.parent == paths.SONARI_DIR
    assert paths.SOCKET_PATH.parent == paths.SONARI_DIR
    assert paths.LOG_PATH.parent == paths.SONARI_DIR


def test_phase2_path_constants_names(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.name == "keymap.json"
    assert paths.HOTKEYD_RESOLVED_PATH.name == "hotkeyd.resolved.json"
    assert paths.HOTKEYD_BIN_PATH.name == "sonari-hotkeyd"


def test_phase2_paths_nested_under_sonari_dir(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.KEYMAP_PATH.parent == paths.SONARI_DIR
    assert paths.HOTKEYD_RESOLVED_PATH.parent == paths.SONARI_DIR
    assert paths.HOTKEYD_BIN_PATH.parent == paths.SONARI_DIR


def test_sonari_dir_is_under_home(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert paths.SONARI_DIR == Path(tmp_path) / ".sonari"


def test_ensure_sonari_dir_creates_directory(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    assert not paths.SONARI_DIR.exists()
    paths.ensure_sonari_dir()
    assert paths.SONARI_DIR.is_dir()


def test_ensure_sonari_dir_is_idempotent(monkeypatch, tmp_path):
    paths = _fresh_paths(monkeypatch, tmp_path)
    paths.ensure_sonari_dir()
    paths.ensure_sonari_dir()  # must not raise on an existing dir
    assert paths.SONARI_DIR.is_dir()


# ---------------------------------------------------------------------------
# socket_connectable
# ---------------------------------------------------------------------------

def test_socket_connectable_returns_true_when_connect_succeeds():
    import sonari.paths as paths
    with mock.patch("sonari.paths.socket.socket") as mock_socket_cls:
        mock_sock = mock.MagicMock()
        mock_socket_cls.return_value = mock_sock
        mock_sock.connect.return_value = None
        assert paths.socket_connectable() is True
        mock_sock.close.assert_called_once()


def test_socket_connectable_returns_false_on_oserror():
    import sonari.paths as paths
    with mock.patch("sonari.paths.socket.socket") as mock_socket_cls:
        mock_sock = mock.MagicMock()
        mock_socket_cls.return_value = mock_sock
        mock_sock.connect.side_effect = OSError("refused")
        assert paths.socket_connectable() is False
        mock_sock.close.assert_called_once()


# ---------------------------------------------------------------------------
# repo_root
# ---------------------------------------------------------------------------

def test_repo_root_returns_string():
    import sonari.paths as paths
    root = paths.repo_root()
    assert isinstance(root, str)


def test_repo_root_is_absolute():
    import sonari.paths as paths
    root = paths.repo_root()
    assert os.path.isabs(root)


def test_repo_root_contains_src_subdir():
    """The canonical repo root must have a src/ subdirectory."""
    import sonari.paths as paths
    root = paths.repo_root()
    assert os.path.isdir(os.path.join(root, "src")), (
        f"Expected a src/ directory under repo_root {root!r}"
    )


def test_repo_root_derivation_matches_file_location():
    """repo_root() must be two directory levels above paths.py."""
    import sonari.paths as paths
    paths_file = os.path.abspath(paths.__file__)
    expected = os.path.dirname(os.path.dirname(os.path.dirname(paths_file)))
    assert paths.repo_root() == expected
