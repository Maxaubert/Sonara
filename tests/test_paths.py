import importlib
import os
import socket
from pathlib import Path
from unittest import mock


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


# ---------------------------------------------------------------------------
# socket_connectable
# ---------------------------------------------------------------------------

def test_socket_connectable_returns_true_when_connect_succeeds():
    import echo.paths as paths
    with mock.patch("echo.paths.socket.socket") as mock_socket_cls:
        mock_sock = mock.MagicMock()
        mock_socket_cls.return_value = mock_sock
        mock_sock.connect.return_value = None
        assert paths.socket_connectable() is True
        mock_sock.close.assert_called_once()


def test_socket_connectable_returns_false_on_oserror():
    import echo.paths as paths
    with mock.patch("echo.paths.socket.socket") as mock_socket_cls:
        mock_sock = mock.MagicMock()
        mock_socket_cls.return_value = mock_sock
        mock_sock.connect.side_effect = OSError("refused")
        assert paths.socket_connectable() is False
        mock_sock.close.assert_called_once()


# ---------------------------------------------------------------------------
# repo_root
# ---------------------------------------------------------------------------

def test_repo_root_returns_string():
    import echo.paths as paths
    root = paths.repo_root()
    assert isinstance(root, str)


def test_repo_root_is_absolute():
    import echo.paths as paths
    root = paths.repo_root()
    assert os.path.isabs(root)


def test_repo_root_contains_src_subdir():
    """The canonical repo root must have a src/ subdirectory."""
    import echo.paths as paths
    root = paths.repo_root()
    assert os.path.isdir(os.path.join(root, "src")), (
        f"Expected a src/ directory under repo_root {root!r}"
    )


def test_repo_root_derivation_matches_file_location():
    """repo_root() must be two directory levels above paths.py."""
    import echo.paths as paths
    paths_file = os.path.abspath(paths.__file__)
    expected = os.path.dirname(os.path.dirname(os.path.dirname(paths_file)))
    assert paths.repo_root() == expected
