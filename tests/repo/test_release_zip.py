"""The runtime release zip (packaging/release_zip.py, runtime plan M9)."""
from __future__ import annotations

import hashlib
import importlib.util
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
