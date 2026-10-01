"""VC++ runtime preload (#29): PyWinRT bundles an old MSVCP140.dll inside its
package; whichever engine imports first binds its copy process-wide, and
onnxruntime (Kokoro) crashes inside the old one whenever a native WinRT
voice spoke first. The daemon preloads the SYSTEM runtime before any engine
import so engine order is irrelevant."""
import ctypes

from sonara import daemon
from sonara.platform.windows import process as process_mod


def test_preload_vc_runtime_loads_system_runtime(monkeypatch):
    calls = []
    monkeypatch.setattr(ctypes, "WinDLL",
                        lambda path: calls.append(path), raising=False)
    process_mod.preload_vc_runtime()
    joined = " ".join(str(c).lower() for c in calls)
    assert "msvcp140.dll" in joined
    assert "system32" in joined
    assert "vcruntime140.dll" in joined


def test_preload_vc_runtime_tolerates_missing_dlls(monkeypatch):
    def boom(path):
        raise OSError("The specified module could not be found")
    monkeypatch.setattr(ctypes, "WinDLL", boom, raising=False)
    process_mod.preload_vc_runtime()                      # must not raise


def test_main_preloads_before_any_platform_import():
    # wiring guard: the preload only helps if it runs BEFORE get_platform()
    # (and thus before any winrt/onnxruntime import) in daemon main().
    import inspect
    src = inspect.getsource(daemon.main)
    assert "preload_vc_runtime()" in src
    assert src.index("preload_vc_runtime()") < src.index("get_platform")
