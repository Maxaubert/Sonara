"""The VC++ runtime staged next to onnxruntime.dll (packaging/runtime_dlls.py, #200),
the cached ORT files fetch() hands out (#55), and the one NuGet package both
platforms take them from."""
from __future__ import annotations

import importlib.util
import io
import os
import stat
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


def _nupkg(rd, cache: Path, replace=None) -> str:
    """A stand-in Microsoft.ML.OnnxRuntime nupkg in the cache (so fetch
    downloads nothing), with every platform's members in the NuGet layout;
    its SHA-256. ``replace`` maps a member to the ``ZipInfo``, data written
    in its place."""
    replace = replace or {}
    cache.mkdir(parents=True, exist_ok=True)
    zpath = cache / rd.ORT_PACKAGE
    with zipfile.ZipFile(zpath, "w") as z:
        z.writestr("Microsoft.ML.OnnxRuntime.nuspec", "<package/>")
        z.writestr("runtimes/win-x64/native/onnxruntime_providers_shared.dll", "not needed on CPU")
        for ort in rd.PLATFORMS.values():
            for member, name in ort.files:
                if member in z.namelist():
                    continue
                if member in replace:
                    z.writestr(*replace[member])
                else:
                    z.writestr(member, f"real {name}")
    return rd.sha256(zpath)


def test_a_changed_cached_ort_file_is_extracted_again_from_the_checked_package(tmp_path):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _nupkg(rd, cache)
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
    pinned = _nupkg(rd, cache)
    real_read = zipfile.ZipFile.read

    def stop_at_the_notices(self, name, *a, **kw):
        if str(getattr(name, "filename", name)).endswith("ThirdPartyNotices.txt"):
            raise KeyboardInterrupt
        return real_read(self, name, *a, **kw)

    monkeypatch.setattr(zipfile.ZipFile, "read", stop_at_the_notices)
    with pytest.raises(KeyboardInterrupt):
        rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    assert not (cache / rd.PLATFORMS["win-x64"].folder).exists()
    monkeypatch.setattr(zipfile.ZipFile, "read", real_read)
    out = rd.fetch(cache, url="http://unused.invalid/", expected=pinned)
    for _, name in rd.ORT_FILES:
        assert (out / name).read_text(encoding="utf-8") == f"real {name}"


def test_both_platforms_take_their_files_from_the_one_pinned_nuget_package():
    rd = _module()
    assert rd.ORT_URL == (
        f"https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/{rd.ORT_VERSION}/"
        f"microsoft.ml.onnxruntime.{rd.ORT_VERSION}.nupkg"
    )
    assert rd.ORT_PACKAGE == rd.ORT_URL.rsplit("/", 1)[1]
    assert len(rd.ORT_SHA256) == 64 and int(rd.ORT_SHA256, 16) >= 0
    win, mac = rd.PLATFORMS["win-x64"], rd.PLATFORMS["macos-arm64"]
    assert ("runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll") in win.files
    assert ("runtimes/osx-arm64/native/libonnxruntime.dylib", "libonnxruntime.dylib") in mac.files
    for ort in (win, mac):
        assert ("LICENSE", "onnxruntime-LICENSE.txt") in ort.files
        assert ("ThirdPartyNotices.txt", "onnxruntime-ThirdPartyNotices.txt") in ort.files
    assert win.vc and not mac.vc
    assert win.folder != mac.folder


def test_the_second_platform_reuses_the_cached_package_without_downloading(tmp_path, monkeypatch):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _nupkg(rd, cache)

    def no_download(*a, **kw):
        raise AssertionError("the cached package is reused")

    monkeypatch.setattr(rd.urllib.request, "urlopen", no_download)
    win = rd.fetch(cache, url="http://unused.invalid/", expected=pinned, platform="win-x64")
    mac = rd.fetch(cache, url="http://unused.invalid/", expected=pinned, platform="macos-arm64")
    assert win != mac
    for out, ort in ((win, rd.PLATFORMS["win-x64"]), (mac, rd.PLATFORMS["macos-arm64"])):
        for _, name in ort.files:
            assert (out / name).read_text(encoding="utf-8") == f"real {name}"
    assert [p.name for p in cache.iterdir() if p.is_file()] == [rd.ORT_PACKAGE]


def test_a_package_that_does_not_match_the_pin_is_downloaded_again_and_refused(tmp_path, monkeypatch):
    rd = _module()
    cache = tmp_path / "ort"
    _nupkg(rd, cache)

    monkeypatch.setattr(rd.urllib.request, "urlopen", lambda url, timeout: io.BytesIO(b"tampered"))
    with pytest.raises(SystemExit, match="is not the pinned"):
        rd.fetch(cache, url="http://mirror.invalid/", expected="0" * 64)
    assert not (cache / rd.PLATFORMS["win-x64"].folder).exists()


DYLIB = "runtimes/osx-arm64/native/libonnxruntime.dylib"


def _symlink(name: str, target: str):
    info = zipfile.ZipInfo(name)
    info.create_system = 3
    info.external_attr = (stat.S_IFLNK | 0o777) << 16
    return info, target


def _folder(name: str):
    info = zipfile.ZipInfo(name)
    info.external_attr = 0x10  # MS-DOS directory bit
    return info, ""


@pytest.mark.parametrize("bad", [_symlink(DYLIB, "libonnxruntime.1.28.0.dylib"), _folder(DYLIB)],
                         ids=["symlink", "directory"])
def test_a_member_that_is_not_a_regular_file_is_refused(tmp_path, bad):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _nupkg(rd, cache, replace={DYLIB: bad})
    with pytest.raises(SystemExit, match="is not a regular file"):
        rd.fetch(cache, url="http://unused.invalid/", expected=pinned, platform="macos-arm64")
    assert not (cache / rd.PLATFORMS["macos-arm64"].folder).exists()

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


def test_staging_for_macos_copies_no_vc_runtime(tmp_path, monkeypatch):
    rd = _module()
    cache = tmp_path / "ort"
    pinned = _nupkg(rd, cache)
    monkeypatch.setattr(rd, "ORT_SHA256", pinned)

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
