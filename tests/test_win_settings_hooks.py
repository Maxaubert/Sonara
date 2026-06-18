"""~/.claude/settings.json hook merge/remove (Windows install glue).

Pure stdlib + tmp files — no winrt fakes needed. Runs identically on macOS and
Windows: the merge logic is OS-independent JSON manipulation.
"""
import json
import os

import pytest

from sonari.platform.windows import supervisor as sup
from sonari.platform.windows.supervisor import (
    merge_hooks_into_settings,
    remove_hooks_from_settings,
    settings_has_sonari_hooks,
    settings_has_sonari_plugin,
)

PW = r"C:\Py\pythonw.exe"
HOOK = r"C:\plug\bin\sonari-hook"


def _read(p):
    with open(p, encoding="utf-8") as fh:
        return json.load(fh)


def test_merge_creates_file_with_sonari_hooks(tmp_path):
    sp = str(tmp_path / "settings.json")
    merge_hooks_into_settings(sp, PW, HOOK)
    data = _read(sp)
    md = data["hooks"]["MessageDisplay"][0]["hooks"][0]
    assert md["command"] == PW and md["args"] == [HOOK, "MessageDisplay"]
    assert settings_has_sonari_hooks(sp)


def test_merge_preserves_unrelated_keys_and_hooks(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({
        "theme": "dark",
        "hooks": {"Stop": [{"matcher": "", "hooks": [
            {"type": "command", "command": "other.exe", "args": ["x"]}]}]},
    }), encoding="utf-8")
    merge_hooks_into_settings(str(sp), PW, HOOK)
    data = _read(str(sp))
    assert data["theme"] == "dark"
    stop_cmds = [h["command"] for e in data["hooks"]["Stop"] for h in e["hooks"]]
    assert "other.exe" in stop_cmds and PW in stop_cmds   # unrelated kept, sonari added


def test_merge_is_idempotent(tmp_path):
    sp = str(tmp_path / "settings.json")
    merge_hooks_into_settings(sp, PW, HOOK)
    merge_hooks_into_settings(sp, PW, HOOK)
    data = _read(sp)
    sonari = [h for e in data["hooks"]["MessageDisplay"] for h in e["hooks"]
              if "sonari-hook" in (h.get("command", "") + " ".join(h.get("args", [])))]
    assert len(sonari) == 1   # not duplicated


def test_remove_drops_only_sonari_entries(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [{"matcher": "", "hooks": [
        {"type": "command", "command": "other.exe", "args": ["x"]}]}]}}), encoding="utf-8")
    merge_hooks_into_settings(str(sp), PW, HOOK)
    remove_hooks_from_settings(str(sp), HOOK)
    data = _read(str(sp))
    assert not settings_has_sonari_hooks(str(sp))
    stop_cmds = [h["command"] for e in data["hooks"]["Stop"] for h in e["hooks"]]
    assert stop_cmds == ["other.exe"]   # unrelated survived


def test_invalid_json_aborts_without_clobber(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text("{ not json", encoding="utf-8")
    with pytest.raises(ValueError):
        merge_hooks_into_settings(str(sp), PW, HOOK)
    assert sp.read_text(encoding="utf-8") == "{ not json"   # untouched


# --- M9: the doctor probes must NEVER raise on a hand-malformed settings.json ---

@pytest.mark.parametrize("blob", [
    '{"hooks": ["not", "a", "dict"]}',            # hooks is a list
    '{"hooks": {"Stop": "not-a-list"}}',          # entries not a list
    '{"hooks": {"Stop": ["not-a-dict"]}}',        # entry not a dict
    '{"hooks": {"Stop": [{"hooks": ["x"]}]}}',    # inner hook not a dict
    '{"hooks": {"Stop": [{"hooks": [{"args": "x"}]}]}}',  # args not a list
    '[1, 2, 3]',                                  # top level not an object
    '"just a string"',
    '42',
])
def test_settings_has_sonari_hooks_tolerates_malformed_shapes(tmp_path, blob):
    sp = tmp_path / "settings.json"
    sp.write_text(blob, encoding="utf-8")
    assert settings_has_sonari_hooks(str(sp)) is False   # no exception, just False


@pytest.mark.parametrize("blob", [
    '{"enabledPlugins": ["sonari@sonari"]}',      # list, not a dict
    '{"enabledPlugins": "sonari"}',
    '[1, 2, 3]',
    '"x"',
])
def test_settings_has_sonari_plugin_tolerates_malformed_shapes(tmp_path, blob):
    sp = tmp_path / "settings.json"
    sp.write_text(blob, encoding="utf-8")
    assert settings_has_sonari_plugin(str(sp)) is False


# --- #11: atomic write (must never corrupt the user's shared settings.json) ---
def test_write_settings_failure_preserves_original(tmp_path, monkeypatch):
    p = tmp_path / "settings.json"
    p.write_text('{"keep": 1}\n', encoding="utf-8")
    monkeypatch.setattr(
        json, "dump", lambda *a, **k: (_ for _ in ()).throw(RuntimeError("boom")))
    with pytest.raises(RuntimeError):
        sup._write_settings(str(p), {"new": 2})
    assert p.read_text(encoding="utf-8") == '{"keep": 1}\n'  # not truncated
    leftovers = [f for f in os.listdir(tmp_path) if f != "settings.json"]
    assert leftovers == [], leftovers  # temp cleaned up


# --- #23: structured marker (a look-alike user hook must not be clobbered) ---
def test_settings_has_sonari_hooks_ignores_lookalike_user_hooks(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [{"matcher": "", "hooks": [
        {"type": "command", "command": "my-sonari-hook-wrapper.exe"}]}]}}),
        encoding="utf-8")
    assert settings_has_sonari_hooks(str(sp)) is False


def test_user_hook_containing_sonari_substring_survives_uninstall(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [{"matcher": "", "hooks": [
        {"type": "command", "command": "my-sonari-hook-wrapper.exe", "args": []}]}]}}),
        encoding="utf-8")
    merge_hooks_into_settings(str(sp), PW, HOOK)
    remove_hooks_from_settings(str(sp), HOOK)
    data = _read(str(sp))
    cmds = [h["command"] for e in data.get("hooks", {}).get("Stop", [])
            for h in e["hooks"]]
    assert "my-sonari-hook-wrapper.exe" in cmds


# --- #8: doctor goes red when the baked hook script path no longer exists ---
def test_hooks_doctor_row_red_when_baked_hook_path_missing(tmp_path, monkeypatch):
    sp = tmp_path / "settings.json"
    missing = str(tmp_path / "gone" / "sonari-hook")
    merge_hooks_into_settings(str(sp), PW, missing)
    monkeypatch.setattr(sup, "claude_settings_path", lambda: str(sp))
    name, ok, detail = sup.WinSupervisorBackend().hooks_doctor_row()
    assert ok is False, (name, detail)


# --- #15: invalid/malformed settings.json fails before a Task is registered ---
def test_merge_rejects_malformed_hooks_shape(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": "not-a-dict"}), encoding="utf-8")
    with pytest.raises(ValueError):
        merge_hooks_into_settings(str(sp), PW, HOOK)


def test_install_merges_hooks_before_registering_the_task(tmp_path, monkeypatch):
    b = sup.WinSupervisorBackend()
    calls = []
    monkeypatch.setattr(sup, "task_install",
                        lambda *a, **k: (calls.append("task"), 0)[1])
    monkeypatch.setattr(sup, "merge_hooks_into_settings",
                        lambda *a, **k: calls.append("hooks"))
    monkeypatch.setattr(b, "_place_launcher",
                        lambda *a, **k: (calls.append("launcher"), "x")[1])
    b.install("pythonw.exe", str(tmp_path))
    assert calls.index("hooks") < calls.index("task"), calls
