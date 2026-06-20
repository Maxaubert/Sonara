from __future__ import annotations
import pathlib
from sonara.platform.windows.earcons import default_earcons, _cache


def test_default_earcons_has_6():
    # Clear cache so each test run is fresh
    _cache.clear()
    earcons = default_earcons()
    assert len(earcons) == 6, f"Expected 6 earcons, got {len(earcons)}: {list(earcons)}"
    for name, path in earcons.items():
        assert pathlib.Path(path).exists(), f"Earcon {name!r} path does not exist: {path!r}"
