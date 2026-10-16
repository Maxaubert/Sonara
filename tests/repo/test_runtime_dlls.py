"""The VC++ runtime staged next to onnxruntime.dll (packaging/runtime_dlls.py, #200),
and the cached ORT files fetch() hands out (#55)."""
from __future__ import annotations

import importlib.util
import os
import sys
import zipfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]


def _module():
    spec = importlib.util.spec_from_file_location("runtime_dlls", REPO / "packaging" / "runtime_dlls.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _ort_zip(rd, cache: Path) -> str:
    """A stand-in ORT zip in the cache (so fetch downloads nothing); its SHA-256."""
    cache.mkdir(parents=True)
    zpath = cache / f"onnxruntime-win-x64-{rd.ORT_VERSION}.zip"
    root = f"onnxruntime-win-x64-{rd.ORT_VERSION}/"
    with zipfile.ZipFile(zpath, "w") as z:
        for member, name in rd.ORT_FILES:
            z.writestr(root + member, f"real {name}")
    return rd.sha256(zpath)


def test_a_changed_cached_ort_file_is_extracted_again_from_the_checked_zip(tmp_path):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _ort_zip(rd, cache)
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    # A truncated notices file (a stopped run) and a changed dll.
    (out / "onnxruntime-ThirdPartyNotices.txt").write_text("real onnx", encoding="utf-8")
    (out / "onnxruntime.dll").write_text("patched", encoding="utf-8")
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    for _, name in rd.ORT_FILES:
        assert (out / name).read_text(encoding="utf-8") == f"real {name}"


def test_an_interrupted_extraction_leaves_nothing_a_later_run_accepts(tmp_path, monkeypatch):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _ort_zip(rd, cache)
    real_read = zipfile.ZipFile.read

    def stop_at_the_notices(self, name, *a, **kw):
        if name.endswith("ThirdPartyNotices.txt"):
            raise KeyboardInterrupt
        return real_read(self, name, *a, **kw)

    monkeypatch.setattr(zipfile.ZipFile, "read", stop_at_the_notices)
    with pytest.raises(KeyboardInterrupt):
        rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    assert not (cache / rd.ORT_VERSION).exists()
    monkeypatch.setattr(zipfile.ZipFile, "read", real_read)
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    for _, name in rd.ORT_FILES:
        assert (out / name).read_text(encoding="utf-8") == f"real {name}"


def _crt(tmp_path, versions):
    for name in versions:
        (tmp_path / name).write_bytes(b"MZ fake")
    return lambda p: versions[p.name]


def test_a_redist_at_or_above_the_minimum_is_accepted(tmp_path):
    rd = _module()
    read = _crt(tmp_path, {"msvcp140.dll": (14, 44, 35112, 1), "vcruntime140.dll": rd.MIN_VC_VERSION})
    assert rd.check_vc(tmp_path, read) == {"msvcp140.dll": "14.44.35112.1",
                                          "vcruntime140.dll": ".".join(map(str, rd.MIN_VC_VERSION))}


def test_a_redist_older_than_the_toolset_ort_was_built_with_is_refused(tmp_path):
    rd = _module()
    read = _crt(tmp_path, {"msvcp140.dll": (14, 29, 30139, 0), "vcruntime140.dll": (14, 44, 35112, 1)})
    with pytest.raises(SystemExit, match=r"msvcp140\.dll .*14\.29\.30139\.0.*older than"):
        rd.check_vc(tmp_path, read)


def test_a_dll_without_a_version_is_refused(tmp_path):
    rd = _module()
    read = _crt(tmp_path, {"msvcp140.dll": None, "vcruntime140.dll": (14, 44, 35112, 1)})
    with pytest.raises(SystemExit, match="no file version"):
        rd.check_vc(tmp_path, read)


@pytest.mark.skipif(sys.platform != "win32", reason="reads a Windows DLL's version resource")
def test_file_version_reads_a_real_dll():
    rd = _module()
    v = rd.file_version(Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "kernel32.dll")
    assert v is not None and len(v) == 4 and v[0] >= 6
    assert rd.file_version(REPO / "README.md") is None
