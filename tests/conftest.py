import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

_SRC = _REPO_ROOT / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

# Install fake Windows modules (winrt/winsound/winreg/msvcrt) into sys.modules so
# platform/windows/* imports and unit-tests on macOS/Linux. No-op on real Windows.
import tests._winfakes as _winfakes
_winfakes.install()

import pytest


@pytest.fixture(autouse=True)
def _isolate_sonara_dir(tmp_path, monkeypatch):
    """Redirect every Sonara path to a per-test tmp dir.

    save_config (and anything else that writes under SONARA_DIR) targets
    CONFIG_PATH = ~/.sonara/config.json by default, which lives OUTSIDE the repo
    and is not git-tracked. Without isolation, daemon tests that exercise the
    real save_config() (e.g. the SET_RATE delta path) mutate the developer's
    actual Sonara config as a filesystem side effect. This autouse fixture
    repoints the path constants on every module that imported them so no test
    can ever touch the real ~/.sonara.
    """
    # Do NOT pre-create the directory: several tests (test_config,
    # test_paths, test_cli_uninstall) assert SONARA_DIR does not yet exist and
    # then verify their own code creates it. save_config()/ensure_sonara_dir()
    # create it on demand on first write.
    sonara_dir = tmp_path / ".sonara"

    import sonara.paths as paths

    monkeypatch.setattr(paths, "SONARA_DIR", sonara_dir, raising=False)
    # APP_DIR is SONARA_DIR/"app" bound at import; it is NOT derived live, so
    # patching SONARA_DIR alone leaves it pointing at the real ~/.sonara/app.
    # The uninstall path shutil.rmtree(APP_DIR)s it -- without this repoint, a
    # plain `pytest` run DELETES the developer's live daemon copy (it did).
    monkeypatch.setattr(paths, "APP_DIR", sonara_dir / "app", raising=False)
    monkeypatch.setattr(paths, "CONFIG_PATH", sonara_dir / "config.json", raising=False)
    monkeypatch.setattr(paths, "LOCK_PATH", sonara_dir / "daemon.lock", raising=False)
    # client.send does `from sonara.paths import LOCK_PATH` (a by-value bind), so
    # patching paths.LOCK_PATH alone leaves the client reading the developer's
    # real ~/.sonara/daemon.lock. Repoint the client module's copy too.
    import sonara.client as client_mod
    monkeypatch.setattr(client_mod, "LOCK_PATH", sonara_dir / "daemon.lock", raising=False)
    monkeypatch.setattr(paths, "LOG_PATH", sonara_dir / "speechd.log", raising=False)
    # STOPPED_SENTINEL_PATH is import-time-bound like the rest; without this a
    # lifecycle test would write the developer's real ~/.sonara/stopped and
    # BLOCK the live daemon's respawn paths (#23).
    monkeypatch.setattr(
        paths, "STOPPED_SENTINEL_PATH", sonara_dir / "stopped", raising=False)
    monkeypatch.setattr(paths, "KEYMAP_PATH", sonara_dir / "keymap.json", raising=False)
    monkeypatch.setattr(
        paths, "HOTKEYD_RESOLVED_PATH", sonara_dir / "hotkeyd.resolved.json",
        raising=False)
    monkeypatch.setattr(
        paths, "HOTKEYD_BIN_PATH", sonara_dir / "sonara-hotkeyd", raising=False)
    monkeypatch.setattr(
        paths, "INSTALL_RECORD_PATH", sonara_dir / "install.json", raising=False)
    # CHATTERBOX_* are SONARA_DIR-derived but bound at import time (not live),
    # same trap as APP_DIR above: without repointing them here, a test that
    # doesn't explicitly monkeypatch sonara.chatterbox's copies (which import
    # these by value) would fall back to the real ~/.sonara paths.
    monkeypatch.setattr(
        paths, "CHATTERBOX_VENV", sonara_dir / "chatterbox-venv", raising=False)
    monkeypatch.setattr(
        paths, "CHATTERBOX_HF_CACHE", sonara_dir / "chatterbox" / "hf-cache", raising=False)
    monkeypatch.setattr(
        paths, "CHATTERBOX_VOICES_DIR", sonara_dir / "voices" / "chatterbox", raising=False)

    # sonara.chatterbox imports CHATTERBOX_HF_CACHE/CHATTERBOX_VOICES_DIR by value
    # at import time, so patching paths.* alone leaves its copies pointed at the
    # real ~/.sonara. Repoint the chatterbox module's copies too.
    import sonara.chatterbox as chatterbox

    monkeypatch.setattr(
        chatterbox, "CHATTERBOX_HF_CACHE", sonara_dir / "chatterbox" / "hf-cache", raising=False)
    monkeypatch.setattr(
        chatterbox, "CHATTERBOX_VOICES_DIR", sonara_dir / "voices" / "chatterbox", raising=False)

    # Modules that bound these names at import time need their copies repointed too.
    import sonara.config as config

    monkeypatch.setattr(config, "SONARA_DIR", sonara_dir, raising=False)
    monkeypatch.setattr(config, "CONFIG_PATH", sonara_dir / "config.json", raising=False)

    # keymap.py binds KEYMAP_PATH/HOTKEYD_RESOLVED_PATH/SONARA_DIR by value at
    # import time, so patching paths.* alone does not redirect it. Repoint the
    # keymap module's copies too so no test (e.g. the `keymap` subcommand, which
    # reads load_keymap()) can ever read or write the real ~/.sonara.
    import sonara.keymap as keymap

    monkeypatch.setattr(keymap, "SONARA_DIR", sonara_dir, raising=False)
    monkeypatch.setattr(keymap, "KEYMAP_PATH", sonara_dir / "keymap.json", raising=False)
    monkeypatch.setattr(
        keymap, "HOTKEYD_RESOLVED_PATH", sonara_dir / "hotkeyd.resolved.json",
        raising=False)

    # daemon.py binds LOCK_PATH + SINGLETON_PATH by value at import; main() takes
    # an exclusive flock on SINGLETON_PATH for single-instance. Repoint per-test
    # (each test has a unique sonara_dir) and reset the process-wide held-flock
    # global so a main()-calling test never blocks a later one.
    monkeypatch.setattr(paths, "SINGLETON_PATH", sonara_dir / "daemon.singleton", raising=False)
    import sonara.daemon as daemon
    monkeypatch.setattr(daemon, "LOCK_PATH", sonara_dir / "daemon.lock", raising=False)
    monkeypatch.setattr(daemon, "SINGLETON_PATH", sonara_dir / "daemon.singleton", raising=False)
    monkeypatch.setattr(daemon, "_SINGLETON", None, raising=False)

    yield
