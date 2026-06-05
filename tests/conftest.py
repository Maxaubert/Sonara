import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parent.parent / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

import pytest


@pytest.fixture(autouse=True)
def _isolate_sonari_dir(tmp_path, monkeypatch):
    """Redirect every Sonari path to a per-test tmp dir.

    save_config (and anything else that writes under SONARI_DIR) targets
    CONFIG_PATH = ~/.sonari/config.json by default, which lives OUTSIDE the repo
    and is not git-tracked. Without isolation, daemon tests that exercise the
    real save_config() (e.g. the SET_RATE delta path) mutate the developer's
    actual Sonari config as a filesystem side effect. This autouse fixture
    repoints the path constants on every module that imported them so no test
    can ever touch the real ~/.sonari.
    """
    # Do NOT pre-create the directory: several tests (test_config,
    # test_paths, test_cli_uninstall) assert SONARI_DIR does not yet exist and
    # then verify their own code creates it. save_config()/ensure_sonari_dir()
    # create it on demand on first write.
    sonari_dir = tmp_path / ".sonari"

    import sonari.paths as paths

    monkeypatch.setattr(paths, "SONARI_DIR", sonari_dir, raising=False)
    monkeypatch.setattr(paths, "CONFIG_PATH", sonari_dir / "config.json", raising=False)
    monkeypatch.setattr(paths, "SOCKET_PATH", sonari_dir / "speechd.sock", raising=False)
    monkeypatch.setattr(paths, "LOG_PATH", sonari_dir / "speechd.log", raising=False)

    # Modules that bound these names at import time need their copies repointed too.
    import sonari.config as config

    monkeypatch.setattr(config, "SONARI_DIR", sonari_dir, raising=False)
    monkeypatch.setattr(config, "CONFIG_PATH", sonari_dir / "config.json", raising=False)

    yield
