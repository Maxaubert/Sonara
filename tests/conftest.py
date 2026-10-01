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
        paths, "INSTALL_RECORD_PATH", sonara_dir / "install.json", raising=False)
    # The removed Chatterbox engine's leftovers (#134) are SONARA_DIR-derived
    # but bound at import time, same trap as APP_DIR above: without repointing
    # them, a cleanup test would delete the developer's real ~/.sonara data.
    monkeypatch.setattr(
        paths, "CHATTERBOX_VENV", sonara_dir / "chatterbox-venv", raising=False)
    monkeypatch.setattr(
        paths, "CHATTERBOX_MODEL_CACHE", sonara_dir / "chatterbox", raising=False)
    monkeypatch.setattr(
        paths, "CHATTERBOX_VOICES_DIR", sonara_dir / "voices" / "chatterbox", raising=False)

    # Modules that bound these names at import time need their copies repointed too.
    import sonara.config as config

    monkeypatch.setattr(config, "SONARA_DIR", sonara_dir, raising=False)
    monkeypatch.setattr(config, "CONFIG_PATH", sonara_dir / "config.json", raising=False)

    # keymap.py binds KEYMAP_PATH by value at import time, so patching paths.*
    # alone does not redirect it. Repoint the
    # keymap module's copies too so no test (e.g. the `keymap` subcommand, which
    # reads load_keymap()) can ever read or write the real ~/.sonara.
    import sonara.keymap as keymap

    monkeypatch.setattr(keymap, "KEYMAP_PATH", sonara_dir / "keymap.json", raising=False)

    # sonara.daemon binds LOCK_PATH and daemon/startup binds SINGLETON_PATH by
    # value at import; main() takes an exclusive flock on SINGLETON_PATH for
    # single-instance. Repoint per-test
    # (each test has a unique sonara_dir) and reset the process-wide held-flock
    # global so a main()-calling test never blocks a later one.
    monkeypatch.setattr(paths, "SINGLETON_PATH", sonara_dir / "daemon.singleton", raising=False)
    import sonara.daemon as daemon
    import sonara.daemon.startup as daemon_startup
    monkeypatch.setattr(daemon, "LOCK_PATH", sonara_dir / "daemon.lock", raising=False)
    monkeypatch.setattr(daemon_startup, "SINGLETON_PATH", sonara_dir / "daemon.singleton")
    monkeypatch.setattr(daemon_startup, "_SINGLETON", None)

    # The Claude Code settings.json the hook installer edits lives outside
    # ~/.sonara (~/.claude/settings.json). A test that reaches install_hooks
    # without its own patch must never rewrite the developer's real one.
    import sonara.install.claude_hooks as claude_hooks
    monkeypatch.setattr(claude_hooks, "claude_settings_path",
                        lambda: str(tmp_path / ".claude" / "settings.json"))

    yield


@pytest.fixture(autouse=True)
def _never_sweep_the_live_daemon(monkeypatch):
    """Stop the suite from killing the DEVELOPER'S running daemon.

    install.service.stop_sonara() ends with the supervisor's
    kill_stray_daemons(), whose default runner shells out to PowerShell and
    Stop-Process -Force's every `-m sonara.daemon` process on the MACHINE (#65). No path repoint can contain that: it matches
    on process command line, not on SONARA_DIR. Any test that drives
    stop_sonara() down the "socket not connectable" branch therefore killed the
    live daemon mid-run -- `sonara status` then reported "not running" with no
    stop sentinel and no crash trace to explain it.

    The guard is deliberately narrow: an EXPLICIT runner still reaches the real
    implementation, so the tests that actually cover kill_stray_daemons
    (test_daemon_state_persistence) keep exercising its counting and
    failure-swallowing logic unchanged. Only the implicit, real-PowerShell path
    is refused.
    """
    import sonara.platform.windows.supervisor as supervisor

    real = supervisor.kill_stray_daemons

    def guarded(runner=None):
        if runner is None:
            return 0        # refuse the machine-wide sweep
        return real(runner=runner)

    monkeypatch.setattr(supervisor, "kill_stray_daemons", guarded)


@pytest.fixture(autouse=True)
def _never_touch_the_real_autostart_or_launcher(tmp_path, monkeypatch):
    """Keep a test that reaches the REAL Windows supervisor (a missed platform
    patch) away from this machine's install: the Task Scheduler task and
    ~/.local/bin/sonara.cmd live outside ~/.sonara, so the path isolation above
    does not cover them. A mutating schtasks call is refused (read-only
    /query still runs), and the launcher directory is a tmp dir. Tests that
    exercise these paths patch subprocess.call or _local_bin_dir themselves,
    which overrides this guard."""
    import subprocess

    import sonara.platform.windows.supervisor as supervisor

    real_call = subprocess.call

    def guarded_call(args, *a, **k):
        argv = [str(x).lower() for x in (args if isinstance(args, (list, tuple)) else [args])]
        if argv and argv[0].endswith("schtasks") and "/query" not in argv:
            return 1        # refuse /create, /delete, /end, /run on the real box
        return real_call(args, *a, **k)

    monkeypatch.setattr(subprocess, "call", guarded_call)
    monkeypatch.setattr(supervisor, "_local_bin_dir", lambda: str(tmp_path / "local-bin"))


def pytest_collection_modifyitems(config, items):
    """Skip @pytest.mark.live_windows tests unless explicitly selected.

    They drive real OneCore / winsound, so they depend on the machine's voices
    and audio stack (E22). Run them with: python -m pytest -m live_windows
    """
    if "live_windows" in (config.getoption("markexpr") or ""):
        return
    skip = pytest.mark.skip(reason="live Windows check; select with -m live_windows")
    for item in items:
        if "live_windows" in item.keywords:
            item.add_marker(skip)
