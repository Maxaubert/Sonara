"""The runtime release zip (packaging/release_zip.py, runtime plan M9)."""
from __future__ import annotations

import hashlib
import importlib.util
import sys
import tarfile
import zipfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]


def _module():
    spec = importlib.util.spec_from_file_location("release_zip", REPO / "packaging" / "release_zip.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


EXES = ("sonarad.exe", "sonara-hook.exe", "sonara.exe")


def _exes(build):
    for name in EXES:
        (build / name).write_bytes(b"MZ " + name.encode())
    return build / "sonarad.exe"


def test_zip_holds_the_runtime_and_the_notices_under_one_folder(tmp_path):
    rz = _module()
    build = tmp_path / "release"
    build.mkdir()
    exe = _exes(build)
    shipped = (
        "onnxruntime.dll", "onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt",
        "msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll", "vcruntime140_1.dll",
    )
    for f in shipped:
        (build / f).write_bytes(b"runtime file")
    (build / "other.dll").write_bytes(b"not shipped")
    (build / "onnxruntime_providers_shared.dll").write_bytes(b"not needed on CPU")
    path = rz.build_zip(exe, tmp_path / "dist", "1.2.3")
    assert path.name == "sonara-runtime-win-x64-1.2.3.zip"
    with zipfile.ZipFile(path) as z:
        names = sorted(z.namelist())
        assert z.read("sonara-runtime-win-x64-1.2.3/sonarad.exe") == b"MZ sonarad.exe"
        assert z.read("sonara-runtime-win-x64-1.2.3/sonara-hook.exe") == b"MZ sonara-hook.exe"
    assert names == sorted(
        f"sonara-runtime-win-x64-1.2.3/{n}"
        for n in (*EXES, *shipped, "LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md")
    )


def test_the_plugin_needs_the_hook_and_the_cli_in_the_zip(tmp_path):
    # #202: the plugin's launcher runs sonara-hook.exe, its commands sonara.exe.
    build = tmp_path / "release"
    build.mkdir()
    exe = _exes(build)
    for f in ("onnxruntime.dll", "onnxruntime-LICENSE.txt"):
        (build / f).write_bytes(b"x")
    (build / "sonara.exe").unlink()
    with pytest.raises(SystemExit, match="sonara.exe"):
        _module().build_zip(exe, tmp_path / "dist", "1.2.3")


def test_sha256sums_lists_the_zip_for_the_bootstrap(tmp_path):
    # bin/sonara-bootstrap.ps1 checks the download against this file.
    rz = _module()
    zip_path = tmp_path / "sonara-runtime-win-x64-1.2.3.zip"
    zip_path.write_bytes(b"zip bytes")
    sums = rz.write_sums([zip_path], tmp_path)
    assert sums == tmp_path / "SHA256SUMS"
    want = hashlib.sha256(b"zip bytes").hexdigest()
    assert sums.read_bytes() == f"{want}  sonara-runtime-win-x64-1.2.3.zip\n".encode()


def test_the_zip_needs_onnx_runtime_for_kokoro(tmp_path):
    build = tmp_path / "release"
    build.mkdir()
    exe = _exes(build)
    with pytest.raises(SystemExit, match="runtime_dlls.py stage"):
        _module().build_zip(exe, tmp_path / "dist", "1.2.3")


def test_a_missing_exe_is_a_clear_error(tmp_path):
    with pytest.raises(SystemExit, match="cargo build -p sonarad --release"):
        _module().build_zip(tmp_path / "sonarad.exe", tmp_path / "dist", "1.2.3")


def test_the_zip_version_is_the_workspace_version():
    from test_manifests import _release_version
    assert _module().workspace_version() == _release_version()


def test_the_notices_ship_with_a_licensing_summary():
    # R6: bundlers may sell, close-source, relicense and sign; they ship the notices.
    text = (REPO / "LICENSING.md").read_text(encoding="utf-8")
    for word in ("sell", "closed-source", "licence", "code-sign", "THIRD_PARTY_NOTICES.md"):
        assert word in text, word
    notices = (REPO / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")
    for needle in ("Kokoro-82M", "misaki", "ONNX Runtime", "Visual C++", "### Apache-2.0", "### MIT"):
        assert needle in notices, needle
    assert "—" not in text + notices


MACOS_EXES = ("sonarad", "sonara-hook", "sonara")


def _macos_build(tmp_path):
    build = tmp_path / "release"
    build.mkdir()
    for name in MACOS_EXES:
        (build / name).write_bytes(b"\xcf\xfa\xed\xfe " + name.encode())
    for f in ("libonnxruntime.dylib", "onnxruntime-LICENSE.txt", "onnxruntime-ThirdPartyNotices.txt"):
        (build / f).write_bytes(b"runtime file")
    (build / "sonarad.d").write_bytes(b"not shipped")
    return build / "sonarad"


def test_the_macos_archive_is_a_tar_gz_that_keeps_the_exec_bits(tmp_path):
    rz = _module()
    path = rz.build_archive(_macos_build(tmp_path), tmp_path / "dist", "1.2.3", "macos-arm64")
    assert path.name == "sonara-runtime-macos-arm64-1.2.3.tar.gz"
    top = "sonara-runtime-macos-arm64-1.2.3"
    with tarfile.open(path, "r:gz") as t:
        members = {m.name: m for m in t.getmembers()}
        assert t.extractfile(f"{top}/sonara-hook").read() == b"\xcf\xfa\xed\xfe sonara-hook"
    files = sorted(n for n, m in members.items() if m.isfile())
    assert files == sorted(
        f"{top}/{n}" for n in (*MACOS_EXES, "libonnxruntime.dylib", "onnxruntime-LICENSE.txt",
                               "onnxruntime-ThirdPartyNotices.txt", "LICENSE", "LICENSING.md",
                               "THIRD_PARTY_NOTICES.md"))
    for n in (*MACOS_EXES, "libonnxruntime.dylib"):
        assert members[f"{top}/{n}"].mode == 0o755, n
    assert members[f"{top}/LICENSE"].mode == 0o644
    assert all(m.isfile() or m.isdir() for m in members.values()), "no links"
    assert not [n for n in members if "/._" in n or n.startswith("._")], "no AppleDouble files"
    assert {m.uname for m in members.values()} == {""}


def test_the_macos_archive_needs_the_dylib_for_kokoro(tmp_path):
    exe = _macos_build(tmp_path)
    (exe.parent / "libonnxruntime.dylib").unlink()
    with pytest.raises(SystemExit, match="runtime_dlls.py stage"):
        _module().build_archive(exe, tmp_path / "dist", "1.2.3", "macos-arm64")


def test_the_default_platform_is_this_machines():
    rz = _module()
    assert rz.host_platform() == ("macos-arm64" if sys.platform == "darwin" else "win-x64")
    assert rz.archive_name("win-x64", "1.2.3") == "sonara-runtime-win-x64-1.2.3.zip"
    assert rz.archive_name("macos-arm64", "1.2.3") == "sonara-runtime-macos-arm64-1.2.3.tar.gz"


def test_one_sha256sums_lists_both_platform_archives(tmp_path):
    # One release serves both platforms: its SHA256SUMS lists both assets.
    rz = _module()
    win, mac, dist = tmp_path / "win", tmp_path / "mac", tmp_path / "dist"
    for d in (win, mac, dist):
        d.mkdir()
    (win / "sonara-runtime-win-x64-1.2.3.zip").write_bytes(b"zip")
    (mac / "sonara-runtime-macos-arm64-1.2.3.tar.gz").write_bytes(b"tgz")
    rz.write_sums([win / "sonara-runtime-win-x64-1.2.3.zip"], win)
    rz.write_sums([mac / "sonara-runtime-macos-arm64-1.2.3.tar.gz"], mac)
    (dist / "sonara-runtime-macos-arm64-1.2.3.tar.gz").write_bytes(b"tgz")
    sums = rz.merge_sums([win / "SHA256SUMS", mac / "SHA256SUMS"], dist)

    def h(b):
        return hashlib.sha256(b).hexdigest()

    assert sums.read_text(encoding="ascii") == (
        f"{h(b'tgz')}  sonara-runtime-macos-arm64-1.2.3.tar.gz\n"
        f"{h(b'zip')}  sonara-runtime-win-x64-1.2.3.zip\n")


def test_merging_refuses_a_hash_its_asset_does_not_have(tmp_path):
    rz = _module()
    a, b = tmp_path / "a", tmp_path / "b"
    a.mkdir()
    b.mkdir()
    (a / "x.zip").write_bytes(b"one")
    (b / "x.zip").write_bytes(b"two")
    rz.write_sums([a / "x.zip"], a)
    rz.write_sums([b / "x.zip"], b)
    with pytest.raises(SystemExit, match="two different"):
        rz.merge_sums([a / "SHA256SUMS", b / "SHA256SUMS"], tmp_path)
    with pytest.raises(SystemExit, match="not the one SHA256SUMS lists"):
        rz.merge_sums([a / "SHA256SUMS"], b)
