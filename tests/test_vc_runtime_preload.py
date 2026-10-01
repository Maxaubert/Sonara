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


def test_main_preamble_imports_no_engine_before_preload():
    # #29 ordering, checked for real: importing the daemon and the process
    # hardening module that main() uses before preload_vc_runtime() must not
    # load any speech backend, so no winrt/onnxruntime/pycaw import can sneak
    # in ahead of the system VC runtime, now or after a future top-level
    # native import in one of the windows/* backend modules.
    import os
    import subprocess
    import sys
    from pathlib import Path
    code = (
        "import sys\n"
        "import sonara.daemon\n"
        "from sonara.platform.windows import process\n"
        "bad = sorted(m for m in sys.modules if m.split('.')[0] in "
        "('winrt', 'onnxruntime', 'pycaw', 'comtypes', 'winsound') or "
        "m.startswith(('sonara.platform.windows.tts', "
        "'sonara.platform.windows.earcon', 'sonara.speaker')))\n"
        "print(','.join(bad))\n"
    )
    src = str(Path(__file__).resolve().parents[1] / "src")
    env = dict(os.environ)
    env["PYTHONPATH"] = src + os.pathsep + env.get("PYTHONPATH", "")
    out = subprocess.run([sys.executable, "-c", code], env=env,
                         capture_output=True, text=True, timeout=60)
    assert out.returncode == 0, out.stderr
    assert out.stdout.strip() == ""
