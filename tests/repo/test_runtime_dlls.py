"""The VC++ runtime staged next to onnxruntime.dll (packaging/runtime_dlls.py, #200),
and the cached ORT files fetch() hands out (#55)."""
from __future__ import annotations

import importlib.util
import io
import os
import sys
import tarfile
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


def _ort_tgz(rd, cache: Path, link: bool = True) -> str:
    """A stand-in macOS ORT tgz in the cache; its SHA-256. With ``link``,
    ``libonnxruntime.dylib`` is a symlink to the versioned file, as some
    ORT releases ship it."""
    cache.mkdir(parents=True, exist_ok=True)
    ort = rd.PLATFORMS["macos-arm64"]
    path = cache / f"{ort.stem}{ort.suffix}"
    root = f"{ort.stem}/"

    def add(t, name, data: bytes):
        info = tarfile.TarInfo(root + name)
        info.size = len(data)
        t.addfile(info, io.BytesIO(data))

    with tarfile.open(path, "w:gz") as t:
        add(t, "LICENSE", b"real onnxruntime-LICENSE.txt")
        add(t, "ThirdPartyNotices.txt", b"real onnxruntime-ThirdPartyNotices.txt")
        if link:
            add(t, f"lib/libonnxruntime.{rd.ORT_VERSION}.dylib", b"real libonnxruntime.dylib")
            info = tarfile.TarInfo(root + "lib/libonnxruntime.dylib")
            info.type = tarfile.SYMTYPE
            info.linkname = f"libonnxruntime.{rd.ORT_VERSION}.dylib"
            t.addfile(info)
        else:
            add(t, "lib/libonnxruntime.dylib", b"real libonnxruntime.dylib")
    return rd.sha256(path)


def test_the_macos_asset_is_the_pinned_osx_arm64_tgz():
    rd = _module()
    ort = rd.PLATFORMS["macos-arm64"]
    assert ort.url.endswith(f"/v{rd.ORT_VERSION}/onnxruntime-osx-arm64-{rd.ORT_VERSION}.tgz")
    assert len(ort.sha256) == 64 and int(ort.sha256, 16) >= 0
    assert ("lib/libonnxruntime.dylib", "libonnxruntime.dylib") in ort.files
    assert not ort.vc
    # The Windows asset is unchanged.
    win = rd.PLATFORMS["win-x64"]
    assert (win.url, win.sha256, win.files, win.folder) == (rd.ORT_URL, rd.ORT_SHA256, rd.ORT_FILES, rd.ORT_VERSION)


@pytest.mark.parametrize("link", [True, False])
def test_the_macos_dylib_is_staged_from_the_checked_tgz_with_its_symlink_resolved(tmp_path, link):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _ort_tgz(rd, cache, link=link)
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned, platform="macos-arm64")
    for _, name in rd.PLATFORMS["macos-arm64"].files:
        assert (out / name).read_text(encoding="utf-8") == f"real {name}"
        assert not (out / name).is_symlink()
    # A changed cached file is extracted again (#55), as on Windows.
    (out / "libonnxruntime.dylib").write_text("patched", encoding="utf-8")
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned, platform="macos-arm64")
    assert (out / "libonnxruntime.dylib").read_text(encoding="utf-8") == "real libonnxruntime.dylib"


def test_staging_for_macos_copies_no_vc_runtime(tmp_path, monkeypatch):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _ort_tgz(rd, cache)
    mac = rd.PLATFORMS["macos-arm64"]
    monkeypatch.setitem(rd.PLATFORMS, "macos-arm64", mac._replace(sha256=pinned))

    def no_vc():
        raise AssertionError("the VC++ runtime is Windows only")

    monkeypatch.setattr(rd, "vc_redist_dir", no_vc)
    dest = tmp_path / "release"
    names = rd.stage(dest, cache=cache, platform="macos-arm64")
    assert names == ["libonnxruntime.dylib", "onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt"]
    assert sorted(p.name for p in dest.iterdir()) == sorted(names)


def test_the_default_platform_is_this_machine():
    rd = _module()
    assert rd.host_platform() == ("macos-arm64" if sys.platform == "darwin" else "win-x64")
