"""The runtime release zip (packaging/release_zip.py, runtime plan M9)."""
from __future__ import annotations

import importlib.util
import zipfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parent.parent


def _module():
    spec = importlib.util.spec_from_file_location("release_zip", REPO / "packaging" / "release_zip.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_zip_holds_the_runtime_and_the_notices_under_one_folder(tmp_path):
    rz = _module()
    build = tmp_path / "release"
    build.mkdir()
    exe = build / "sonarad.exe"
    exe.write_bytes(b"MZ fake")
    (build / "onnxruntime.dll").write_bytes(b"dll")
    (build / "other.dll").write_bytes(b"not shipped")
    path = rz.build_zip(exe, tmp_path / "dist", "1.2.3")
    assert path.name == "sonara-runtime-win-x64-1.2.3.zip"
    with zipfile.ZipFile(path) as z:
        names = sorted(z.namelist())
        assert z.read("sonara-runtime-win-x64-1.2.3/sonarad.exe") == b"MZ fake"
    assert names == sorted(
        f"sonara-runtime-win-x64-1.2.3/{n}"
        for n in ("sonarad.exe", "onnxruntime.dll", "LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md")
    )


def test_a_missing_exe_is_a_clear_error(tmp_path):
    with pytest.raises(SystemExit, match="cargo build -p sonarad --release"):
        _module().build_zip(tmp_path / "sonarad.exe", tmp_path / "dist", "1.2.3")


def test_the_zip_version_is_the_workspace_version():
    from test_manifests import _pyproject_version
    assert _module().workspace_version() == _pyproject_version()


def test_the_notices_ship_with_a_licensing_summary():
    # R6: bundlers may sell, close-source, relicense and sign; they ship the notices.
    text = (REPO / "LICENSING.md").read_text(encoding="utf-8")
    for word in ("sell", "closed-source", "licence", "code-sign", "THIRD_PARTY_NOTICES.md"):
        assert word in text, word
    notices = (REPO / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")
    for needle in ("Kokoro-82M", "misaki", "ONNX Runtime", "Visual C++", "### Apache-2.0", "### MIT"):
        assert needle in notices, needle
    assert "—" not in text + notices
