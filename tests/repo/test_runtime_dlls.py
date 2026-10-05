"""The VC++ runtime staged next to onnxruntime.dll (packaging/runtime_dlls.py, #200)."""
from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]


def _module():
    spec = importlib.util.spec_from_file_location("runtime_dlls", REPO / "packaging" / "runtime_dlls.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


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
