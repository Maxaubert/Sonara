"""Mock-based tests for WinSupervisorBackend, Task XML, resolve_python.

WINDOWS-only code, exercised on macOS via the _winfakes harness (winreg fake is
installed by tests/conftest.py before this module imports the backend). "Green"
here means the MOCKED contract holds, NOT that it works on Windows. The real gate
is docs/superpowers/M2-WINDOWS-ACCEPTANCE.md.

Inject a fake winreg module before importing the Windows backend, then monkeypatch
instance methods for all external calls. XML structure is validated via
ElementTree.fromstring() with the full namespace string, which is more robust than
string-contains checks. The sys.modules.setdefault call is idempotent — running on
real Windows leaves the genuine winreg intact.
"""
import sys
import types
import xml.etree.ElementTree as ET

# --- winreg injection (must happen before any import of the windows backend) ---
if sys.platform != "win32":
    _fake_winreg = types.ModuleType("winreg")
    _fake_winreg.HKEY_LOCAL_MACHINE = 0x80000002
    _fake_winreg.OpenKey = lambda *a, **kw: None
    _fake_winreg.EnumKey = lambda *a, **kw: (_ for _ in ()).throw(OSError())
    _fake_winreg.QueryValueEx = lambda *a, **kw: (_ for _ in ()).throw(OSError())
    sys.modules.setdefault("winreg", _fake_winreg)

from sonara.platform.windows import supervisor as sup_mod
from sonara.platform.windows.supervisor import (
    WinSupervisorBackend, TASK_NAME, TASK_XML_TEMPLATE, _SPAWN_FLAGS,
    daemon_pythonw,
)

_NS = "http://schemas.microsoft.com/windows/2004/02/mit/task"


def test_task_xml_logon_trigger_user_id():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id="DESKTOP-ABC\\nima",
        pythonw=r"C:\Python311\pythonw.exe",
        supervisor_py=r"C:\sonara\supervisor_loop.py",
        work_dir=r"C:\sonara",
    )
    root = ET.fromstring(xml_str)
    uid_el = root.find(f".//{{{_NS}}}LogonTrigger/{{{_NS}}}UserId")
    assert uid_el is not None and uid_el.text == "DESKTOP-ABC\\nima"


def test_task_xml_restart_on_failure_present():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id="DESKTOP\\u", pythonw="pw.exe",
        supervisor_py="s.py", work_dir=".",
    )
    root = ET.fromstring(xml_str)
    rof = root.find(f".//{{{_NS}}}RestartOnFailure")
    assert rof is not None
    interval = rof.find(f"{{{_NS}}}Interval")
    assert interval.text == "PT5M"


def test_task_xml_run_level_least_privilege():
    xml_str = TASK_XML_TEMPLATE.format(
        user_id="U", pythonw="pw.exe", supervisor_py="s.py", work_dir=".",
    )
    root = ET.fromstring(xml_str)
    rl = root.find(f".//{{{_NS}}}Principal/{{{_NS}}}RunLevel")
    assert rl.text == "LeastPrivilege"


def test_launch_spec_creationflags(monkeypatch):
    monkeypatch.setattr(sup_mod, "daemon_pythonw", lambda: r"C:\Python311\pythonw.exe")
    argv, kwargs = WinSupervisorBackend().launch_spec()
    assert argv[0].endswith("pythonw.exe")
    assert argv[-1] == "sonara.daemon"
    flags = kwargs["creationflags"]
    assert flags & 0x08000000, "CREATE_NO_WINDOW must be set"
    assert flags & 0x00000008, "DETACHED_PROCESS must be set"
    assert not kwargs.get("start_new_session", False), "must NOT combine with DETACHED_PROCESS"
    kwargs["stderr"].close()


def test_launch_spec_sets_pythonpath_to_src(monkeypatch):
    # The lazily-spawned daemon runs `pythonw -m sonara.daemon` in a fresh
    # process; without PYTHONPATH it cannot import sonara, dies instantly, and
    # every hook event respawns it -> a relaunch storm. The spawn env must put
    # the repo's src/ first on PYTHONPATH.
    import os
    from sonara import paths

    monkeypatch.setattr(sup_mod, "daemon_pythonw", lambda: r"C:\Python311\pythonw.exe")
    argv, kwargs = WinSupervisorBackend().launch_spec()
    env = kwargs.get("env")
    assert env is not None, "launch_spec must pass an env so the daemon can import sonara"
    src = os.path.join(paths.repo_root(), "src")
    assert env.get("PYTHONPATH", "").split(os.pathsep)[0] == src
    kwargs["stderr"].close()


def test_launch_spec_routes_stderr_to_log_file_not_devnull(tmp_path, monkeypatch):
    """The lazily-spawned daemon's stderr must land in the daemon log under
    SONARA_DIR (paths.LOG_PATH) rather than subprocess.DEVNULL, so the speak-loop
    catch-all traceback survives on Windows. Mirrors the macOS plist
    StandardErrorPath. Regression for #20. stdin/stdout stay DEVNULL."""
    import subprocess
    from sonara import paths

    log = tmp_path / "speechd.log"
    monkeypatch.setattr(paths, "SONARA_DIR", tmp_path)
    monkeypatch.setattr(paths, "LOG_PATH", log)
    monkeypatch.setattr(sup_mod, "daemon_pythonw", lambda: r"C:\Python311\pythonw.exe")

    argv, kwargs = WinSupervisorBackend().launch_spec()
    assert kwargs["stderr"] is not subprocess.DEVNULL
    assert str(kwargs["stderr"].name) == str(log)
    assert kwargs["stdin"] is subprocess.DEVNULL
    assert kwargs["stdout"] is subprocess.DEVNULL
    kwargs["stderr"].close()


# ---------------------------------------------------------------------------
# FIX B: daemon_pythonw() + launch_spec neural-aware resolver
# ---------------------------------------------------------------------------

def test_daemon_pythonw_prefers_venv_pythonw_when_neural(monkeypatch):
    from sonara import kokoro_provision, paths
    monkeypatch.setattr(kokoro_provision, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/v/Scripts/python.exe")
    monkeypatch.setattr(sup_mod, "_probe_python_version", lambda c: (3, 12))
    monkeypatch.setattr(sup_mod, "_find_pythonw", lambda p: "/v/Scripts/pythonw.exe")
    assert daemon_pythonw() == "/v/Scripts/pythonw.exe"


def test_daemon_pythonw_falls_back_to_system_when_no_neural(monkeypatch):
    from sonara import kokoro_provision
    monkeypatch.setattr(kokoro_provision, "neural_enabled", lambda: False)
    monkeypatch.setattr(sup_mod, "resolve_python_windows", lambda: r"C:\sys\pythonw.exe")
    assert daemon_pythonw() == r"C:\sys\pythonw.exe"


def test_daemon_pythonw_falls_back_when_venv_too_old(monkeypatch):
    from sonara import kokoro_provision, paths
    monkeypatch.setattr(kokoro_provision, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/v/Scripts/python.exe")
    monkeypatch.setattr(sup_mod, "_probe_python_version", lambda c: (3, 9))
    monkeypatch.setattr(sup_mod, "resolve_python_windows", lambda: "sys-pw")
    assert daemon_pythonw() == "sys-pw"


def test_launch_spec_uses_daemon_pythonw(tmp_path, monkeypatch):
    from sonara import paths
    monkeypatch.setattr(sup_mod, "daemon_pythonw", lambda: "/v/Scripts/pythonw.exe")
    monkeypatch.setattr(paths, "repo_root", lambda: str(tmp_path))
    argv, kwargs = WinSupervisorBackend().launch_spec()
    assert argv[0] == "/v/Scripts/pythonw.exe"
    kwargs["stderr"].close()


def test_is_installed_calls_schtasks_query(monkeypatch):
    sup = WinSupervisorBackend()
    calls = []
    monkeypatch.setattr(sup, "_schtasks", lambda args: calls.append(args) or 0)
    assert sup.is_installed() is True
    assert calls[0] == ["/query", "/tn", TASK_NAME]


def test_doctor_rows_include_task_and_neural_voice(monkeypatch):
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, "_schtasks", lambda args: 0)
    monkeypatch.setattr(sup, "resolve_python", lambda: r"C:\Python311\pythonw.exe")
    monkeypatch.setattr(sup, "_list_neural_voices", lambda: ["Microsoft Aria Online"])
    monkeypatch.setattr("sonara.paths.socket_connectable", lambda: True)
    names = [r[0] for r in sup.doctor_rows()]
    assert "Task Scheduler task" in names
    assert "pythonw.exe" in names
    assert "neural voice" in names
    assert "daemon running" in names


def test_doctor_row_flags_missing_winrt(monkeypatch):
    # PyWinRT absent => total no-speech; doctor must go RED, not stay green. (#7)
    sup = WinSupervisorBackend()
    monkeypatch.setattr(sup, "_schtasks", lambda args: 0)
    monkeypatch.setattr(sup, "resolve_python", lambda: r"C:\Py\pythonw.exe")
    monkeypatch.setattr(sup, "_list_neural_voices", lambda: ["X"])
    monkeypatch.setattr("sonara.paths.socket_connectable", lambda: True)
    import sonara.platform.windows.tts as tts
    monkeypatch.setattr(tts, "_winrt_available", lambda: False, raising=False)
    rows = {r[0]: r for r in sup.doctor_rows()}
    assert "TTS runtime" in rows
    assert rows["TTS runtime"][1] is False
    assert "winrt" in rows["TTS runtime"][2].lower()


def test_resolve_python_skips_store_stub(monkeypatch, tmp_path):
    # Verify _is_store_stub fast-path (WindowsApps in path)
    from sonara.platform.windows.supervisor import _is_store_stub
    stub = str(tmp_path / "WindowsApps" / "python.exe")
    assert _is_store_stub(stub) is True


def test_spawn_flags_value():
    # Hex literal correctness — no subprocess import needed
    assert _SPAWN_FLAGS == 0x08000008


def test_post_install_notes_are_accurate(capsys):
    # #19: hotkeys ship + start with the daemon, so don't say they "arrive in M3".
    # The plugin's command files were renamed to NTFS-safe names (status.md, ...),
    # so the /sonara:* slash commands DO work on Windows now — the old #10 "not
    # available on NTFS" note is obsolete after the cross-platform-commands fix, and
    # promising a command that doesn't exist would be the inaccuracy to avoid.
    WinSupervisorBackend().post_install_notes()
    out = capsys.readouterr().out
    low = out.lower()
    assert "sonara doctor" in out
    assert "milestone 3" not in low and "arrive in" not in low   # #19
    assert "hotkey" in low                                        # hotkeys are active now
    assert "slash command" in low                                # available via the plugin
    assert "not available" not in low and "aren't available" not in low


def test_post_install_notes_runs(capsys):
    from sonara.platform.windows.supervisor import WinSupervisorBackend
    WinSupervisorBackend().post_install_notes()
    out = capsys.readouterr().out
    assert "sonara doctor" in out   # next steps (accuracy covered by test_post_install_notes_are_accurate)


def test_hooks_doctor_row_windows_absent(monkeypatch, tmp_path):
    from sonara.platform.windows import supervisor as sup
    monkeypatch.setattr(sup, "claude_settings_path",
                        lambda: str(tmp_path / "settings.json"))
    name, ok, _ = sup.WinSupervisorBackend().hooks_doctor_row()
    assert name == "hooks installed" and ok is False   # no settings, no plugin


def test_hooks_doctor_row_ok_when_plugin_enabled(monkeypatch, tmp_path):
    # Hooks supplied by the enabled plugin (no hand-wired settings.json block) must
    # pass — not report FAIL as if uninstalled.
    import json
    from sonara.platform.windows import supervisor as sup
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"enabledPlugins": {"sonara@sonara": True}}), encoding="utf-8")
    monkeypatch.setattr(sup, "claude_settings_path", lambda: str(sp))
    name, ok, detail = sup.WinSupervisorBackend().hooks_doctor_row()
    assert name == "hooks installed" and ok is True
    assert "plugin" in detail


def test_settings_has_sonara_plugin(tmp_path):
    import json
    from sonara.platform.windows import supervisor as sup
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"enabledPlugins": {"x@mkt": True, "sonara@sonara": True}}),
                  encoding="utf-8")
    assert sup.settings_has_sonara_plugin(str(sp))
    sp.write_text(json.dumps({"enabledPlugins": {"sonara@sonara": False}}), encoding="utf-8")
    assert not sup.settings_has_sonara_plugin(str(sp))   # disabled doesn't count
    sp.write_text(json.dumps({"enabledPlugins": {}}), encoding="utf-8")
    assert not sup.settings_has_sonara_plugin(str(sp))


def test_install_registers_task_merges_hooks_and_places_launcher(tmp_path, monkeypatch):
    from sonara.platform.windows import supervisor as sup
    calls = []
    monkeypatch.setattr(sup, "task_install", lambda pw, spy: calls.append(("task", pw)) or 0)
    monkeypatch.setattr(sup, "claude_settings_path",
                        lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonara.paths.repo_root", lambda: str(tmp_path / "plug"))
    s = sup.WinSupervisorBackend()
    monkeypatch.setattr(s, "_schtasks", lambda args: 0)  # FIX E adds a _schtasks call
    s.install(r"C:\Py\pythonw.exe", str(tmp_path / "app"))
    assert ("task", r"C:\Py\pythonw.exe") in calls
    assert sup.settings_has_sonara_hooks(str(tmp_path / "settings.json"))
    assert (tmp_path / "bin" / "sonara.cmd").exists()


# ---------------------------------------------------------------------------
# FIX D: install() passes pythonw (not the raw python) to task_install + hooks
# ---------------------------------------------------------------------------

def test_install_wires_task_and_hooks_with_pythonw(tmp_path, monkeypatch):
    task_calls = []
    hook_calls = []
    monkeypatch.setattr(sup_mod, "_find_pythonw", lambda p: "/v/Scripts/pythonw.exe")
    monkeypatch.setattr(sup_mod, "task_install",
                        lambda pw, spy: task_calls.append(pw) or 0)
    monkeypatch.setattr(sup_mod, "merge_hooks_into_settings",
                        lambda sp, pw, hp: hook_calls.append(pw))
    monkeypatch.setattr(sup_mod, "claude_settings_path",
                        lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup_mod, "settings_has_sonara_plugin", lambda sp: False)
    monkeypatch.setattr(sup_mod, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonara.paths.repo_root", lambda: str(tmp_path / "plug"))
    launcher_calls = []
    s = sup_mod.WinSupervisorBackend()
    monkeypatch.setattr(s, "_schtasks", lambda args: 0)  # FIX E adds a _schtasks /end call
    real_place = s._place_launcher
    monkeypatch.setattr(s, "_place_launcher",
                        lambda py, app: launcher_calls.append(py) or real_place(py, app))
    s.install("/v/Scripts/python.exe", str(tmp_path / "app"))
    assert task_calls == ["/v/Scripts/pythonw.exe"], "task_install must receive pythonw"
    assert hook_calls == ["/v/Scripts/pythonw.exe"], "merge_hooks must receive pythonw"
    assert launcher_calls == ["/v/Scripts/python.exe"], "_place_launcher must keep python.exe"


def test_uninstall_removes_task_hooks_and_launcher(tmp_path, monkeypatch):
    from sonara.platform.windows import supervisor as sup
    monkeypatch.setattr(sup, "task_install", lambda pw, spy: 0)
    monkeypatch.setattr(sup, "task_uninstall", lambda: 0)
    monkeypatch.setattr(sup, "claude_settings_path",
                        lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonara.paths.repo_root", lambda: str(tmp_path / "plug"))
    s = sup.WinSupervisorBackend()
    monkeypatch.setattr(s, "_schtasks", lambda args: 0)  # FIX E adds a _schtasks call
    s.install(r"C:\Py\pythonw.exe", str(tmp_path / "app"))
    s.uninstall()
    assert not sup.settings_has_sonara_hooks(str(tmp_path / "settings.json"))
    assert not (tmp_path / "bin" / "sonara.cmd").exists()


# ---------------------------------------------------------------------------
# FIX E: install() stops the running task before re-registering it
# ---------------------------------------------------------------------------

def test_install_ends_task_before_reregister(tmp_path, monkeypatch):
    schtasks_calls = []
    monkeypatch.setattr(sup_mod, "task_install",
                        lambda pw, spy: schtasks_calls.append(("/create", pw)) or 0)
    monkeypatch.setattr(sup_mod, "merge_hooks_into_settings",
                        lambda sp, pw, hp: None)
    monkeypatch.setattr(sup_mod, "claude_settings_path",
                        lambda: str(tmp_path / "settings.json"))
    monkeypatch.setattr(sup_mod, "settings_has_sonara_plugin", lambda sp: False)
    monkeypatch.setattr(sup_mod, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    monkeypatch.setattr("sonara.paths.repo_root", lambda: str(tmp_path / "plug"))
    s = sup_mod.WinSupervisorBackend()
    monkeypatch.setattr(s, "_schtasks", lambda args: schtasks_calls.append(tuple(args)) or 0)
    s.install(r"C:\Py\pythonw.exe", str(tmp_path / "app"))
    # The /end call must appear and must precede any /create
    assert ("/end", "/tn", TASK_NAME) in schtasks_calls
    end_idx = schtasks_calls.index(("/end", "/tn", TASK_NAME))
    create_indices = [i for i, c in enumerate(schtasks_calls) if c[0] == "/create"]
    assert all(end_idx < ci for ci in create_indices), (
        "/end must come before any /create: {0}".format(schtasks_calls)
    )


def test_resolve_python_windows_falls_back_to_recorded(monkeypatch):
    from sonara.platform.windows import supervisor as sup
    from sonara import paths
    # No system Python anywhere.
    monkeypatch.setattr(sup.shutil, "which", lambda name: None)
    monkeypatch.setattr(paths, "recorded_pythonw", lambda: r"C:\uv\pythonw.exe")
    assert sup.resolve_python_windows() == r"C:\uv\pythonw.exe"


def test_resolve_python_windows_none_when_no_system_and_no_record(monkeypatch):
    from sonara.platform.windows import supervisor as sup
    from sonara import paths
    monkeypatch.setattr(sup.shutil, "which", lambda name: None)
    monkeypatch.setattr(paths, "recorded_pythonw", lambda: None)
    assert sup.resolve_python_windows() is None
