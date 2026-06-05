import json
import os
import shutil
import subprocess

import pytest

from sonari import keymap

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SWIFT_SRC = os.path.join(REPO_ROOT, "hotkeyd", "sonari-hotkeyd.swift")


def test_swift_source_exists():
    assert os.path.isfile(SWIFT_SRC), SWIFT_SRC


@pytest.mark.skipif(shutil.which("swiftc") is None, reason="swiftc not available")
def test_swift_source_compiles(tmp_path):
    out = tmp_path / "sonari-hotkeyd"
    proc = subprocess.run(
        ["swiftc", SWIFT_SRC, "-o", str(out)],
        capture_output=True, text=True,
    )
    assert proc.returncode == 0, proc.stderr
    assert out.exists()


def test_resolved_json_shape_matches_swift_contract(monkeypatch, tmp_path):
    # The Swift reads entries with int keyCode, int modifiers, str message.
    resolved = tmp_path / "hotkeyd.resolved.json"
    monkeypatch.setattr(keymap, "HOTKEYD_RESOLVED_PATH", resolved)
    monkeypatch.setattr(keymap, "SONARI_DIR", tmp_path)
    monkeypatch.setattr(keymap, "KEYMAP_PATH", tmp_path / "keymap.json")
    monkeypatch.setattr(keymap, "ensure_sonari_dir",
                        lambda: tmp_path.mkdir(parents=True, exist_ok=True))
    keymap.write_resolved()
    data = json.loads(resolved.read_text(encoding="utf-8"))
    assert isinstance(data, list) and data
    for entry in data:
        assert set(entry.keys()) >= {"keyCode", "modifiers", "message"}
        assert isinstance(entry["keyCode"], int)
        assert isinstance(entry["modifiers"], int)
        assert isinstance(entry["message"], str)
        # message is itself a JSON object speechd understands
        assert isinstance(json.loads(entry["message"]), dict)
