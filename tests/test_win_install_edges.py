"""Windows install edges (#139): settings.json hooks generated from the plugin's
hooks/hooks.json (H4/E17), the Task XML (L-xml, E14), uninstall on a malformed
settings.json (E19), windowless interpreter probes (E9) and one interpreter
probe (L-interp-dup)."""
import json
import os
import xml.etree.ElementTree as ET
from pathlib import Path

from sonara.install import claude_hooks
from sonara import paths
from sonara.platform.windows import supervisor as sup

REPO = Path(__file__).resolve().parent.parent
PLUGIN_HOOKS = json.loads((REPO / "hooks" / "hooks.json").read_text(encoding="utf-8"))["hooks"]


def _pairs(hooks):
    return {(event, e.get("matcher", "")) for event, entries in hooks.items()
            for e in entries}


# --- H4 / E17: one source for the hook set ------------------------------------

def test_settings_hooks_have_the_plugin_hooks_events_and_matchers():
    built = claude_hooks._build_hooks_dict(r"C:\py\pythonw.exe", r"C:\plug\bin\sonara-hook")
    assert set(built) == set(PLUGIN_HOOKS)
    assert _pairs(built) == _pairs(PLUGIN_HOOKS)
    assert "PostToolUse" in built         # CHOICE_ANSWERED needs it (H4)


def test_settings_hooks_are_exec_form_with_the_event_as_last_arg():
    built = claude_hooks._build_hooks_dict(r"C:\py\pythonw.exe", r"C:\plug\bin\sonara-hook")
    for event, entries in built.items():
        for entry in entries:
            for h in entry["hooks"]:
                assert h["command"] == r"C:\py\pythonw.exe"
                assert h["args"] == [r"C:\plug\bin\sonara-hook", event]
                assert h[claude_hooks.SONARA_HOOK_MARKER] is True


def test_settings_hooks_follow_the_given_plugin_roots_hooks_file(tmp_path):
    (tmp_path / "hooks").mkdir()
    (tmp_path / "hooks" / "hooks.json").write_text(json.dumps({"hooks": {"Stop": [
        {"matcher": "", "hooks": [{"type": "command",
                                    "command": '"${CLAUDE_PLUGIN_ROOT}/bin/x" Stop'}]}]}}),
        encoding="utf-8")
    built = claude_hooks._build_hooks_dict("pw", str(tmp_path / "bin" / "sonara-hook"),
                                  plugin_root=str(tmp_path))
    assert list(built) == ["Stop"]


def test_install_bakes_the_hook_of_the_plugin_root_it_was_given(tmp_path, monkeypatch):
    b = sup.WinSupervisorBackend()
    sp = tmp_path / "settings.json"
    sp.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(claude_hooks, "claude_settings_path", lambda: str(sp))
    monkeypatch.setattr(sup, "task_install", lambda *a, **k: 0)
    monkeypatch.setattr(b, "_place_launcher", lambda *a, **k: "x")
    monkeypatch.setattr(b, "_schtasks", lambda args: 0)
    b.install("pythonw.exe", str(tmp_path / "app"), plugin_root=str(REPO))
    data = json.loads(sp.read_text(encoding="utf-8"))
    args = data["hooks"]["Stop"][0]["hooks"][0]["args"]
    assert args[0] == os.path.join(str(REPO), "bin", "sonara-hook")


# --- E19: uninstall on a malformed settings.json -------------------------------

def test_uninstall_survives_a_malformed_settings_json(tmp_path, monkeypatch, capsys):
    sp = tmp_path / "settings.json"
    sp.write_text("{ not json", encoding="utf-8")
    monkeypatch.setattr(claude_hooks, "claude_settings_path", lambda: str(sp))
    monkeypatch.setattr(sup, "task_uninstall", lambda: 0)
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    sup.WinSupervisorBackend().uninstall()          # must not raise
    assert sp.read_text(encoding="utf-8") == "{ not json"   # never clobbered
    assert "settings.json" in capsys.readouterr().out


# --- L-xml / E14: the Task Scheduler XML ---------------------------------------

def _captured_task_xml(monkeypatch, user_id, pythonw, supervisor_py):
    seen = {}

    def fake_call(argv, **kwargs):
        with open(argv[argv.index("/xml") + 1], encoding="utf-16") as fh:
            seen["xml"] = fh.read()
        return 0

    monkeypatch.setattr(sup, "_current_user_id", lambda: user_id)
    monkeypatch.setattr(sup.subprocess, "call", fake_call)
    assert sup.task_install(pythonw, supervisor_py) == 0
    return seen["xml"]


def _ns(tag):
    return "{http://schemas.microsoft.com/windows/2004/02/mit/task}" + tag


def test_task_xml_escapes_values(monkeypatch):
    xml = _captured_task_xml(monkeypatch, r"R&D\o'brien<x>",
                             r"C:\A&B\pythonw.exe",
                             r"C:\A&B\app\sonara\platform\windows\supervisor_loop.py")
    root = ET.fromstring(xml.encode("utf-16"))      # parses: '&' and '<' escaped
    assert root.find(".//" + _ns("UserId")).text == r"R&D\o'brien<x>"
    assert root.find(".//" + _ns("Command")).text == r"C:\A&B\pythonw.exe"


def test_task_working_directory_is_outside_the_swapped_app_tree(monkeypatch):
    # E14: a cwd inside ~/.sonara/app/sonara pins the tree install renames and
    # uninstall deletes.
    xml = _captured_task_xml(monkeypatch, "PC\\me", r"C:\py\pythonw.exe",
                             str(paths.APP_DIR / "sonara" / "platform" / "windows"
                                 / "supervisor_loop.py"))
    root = ET.fromstring(xml.encode("utf-16"))
    assert root.find(".//" + _ns("WorkingDirectory")).text == str(paths.SONARA_DIR)


# --- E9: interpreter probes never flash a console -------------------------------

def test_interpreter_probes_run_windowless(monkeypatch):
    calls = []

    class _Done:
        returncode = 0
        stdout = r"C:\Py\python.exe"

    def fake_run(argv, **kw):
        calls.append(kw)
        return _Done()

    def fake_check_output(argv, **kw):
        calls.append(kw)
        return "3.12" if "version_info" in argv[-1] else r"C:\Py\python.exe"

    monkeypatch.setattr(sup.shutil, "which",
                        lambda name: {"py": r"C:\Windows\py.exe",
                                      "python": r"C:\Py\python.exe"}.get(name))
    monkeypatch.setattr(sup.subprocess, "run", fake_run)
    monkeypatch.setattr(sup.subprocess, "check_output", fake_check_output)
    monkeypatch.setattr(sup, "_find_pythonw", lambda p: r"C:\Py\pythonw.exe")
    assert sup.resolve_python_windows() == r"C:\Py\pythonw.exe"
    assert calls, "no probe ran"
    for kw in calls:
        assert kw.get("creationflags", 0) & 0x08000000, kw


# --- L-interp-dup: one probe, one venv choice ----------------------------------

def test_backend_probe_is_the_module_probe(monkeypatch):
    monkeypatch.setattr(sup, "_probe_python_version", lambda c: (3, 42))
    assert sup.WinSupervisorBackend()._probe_python_version("x") == (3, 42)


def test_cli_and_backend_share_the_venv_choice(monkeypatch):
    from sonara import cli
    from sonara import kokoro_provision as kp
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: r"C:\v\Scripts\python.exe")
    picked = []
    monkeypatch.setattr(kp, "usable_venv_python",
                        lambda probe: picked.append(probe) or r"C:\v\Scripts\python.exe")
    monkeypatch.setattr(sup, "_find_pythonw", lambda p: r"C:\v\Scripts\pythonw.exe")
    assert sup.daemon_pythonw() == r"C:\v\Scripts\pythonw.exe"

    class _Sup:
        def resolve_python(self):
            return "sys"

        def _probe_python_version(self, p):
            return (3, 12)

    assert cli._daemon_python(_Sup()) == r"C:\v\Scripts\python.exe"
    assert len(picked) == 2


def test_supervisor_delegates_the_claude_code_hooks_to_claude_hooks(tmp_path, monkeypatch):
    # The settings.json writer moved to sonara.install.claude_hooks (audit
    # section 2): the supervisor only calls its install, uninstall and doctor
    # steps, hooks first so a bad settings.json leaves no orphaned task.
    calls = []
    monkeypatch.setattr(claude_hooks, "install_hooks",
                        lambda pw, root: calls.append(("hooks", pw, root)))
    monkeypatch.setattr(claude_hooks, "uninstall_hooks",
                        lambda: calls.append(("unhooks",)))
    monkeypatch.setattr(claude_hooks, "doctor_row", lambda: ("hooks installed", True, "x"))
    monkeypatch.setattr(sup, "_find_pythonw", lambda p: r"C:\py\pythonw.exe")
    monkeypatch.setattr(sup, "task_install", lambda *a, **k: calls.append(("task",)) or 0)
    monkeypatch.setattr(sup, "task_uninstall", lambda: 0)
    monkeypatch.setattr(sup, "_local_bin_dir", lambda: str(tmp_path / "bin"))
    b = sup.WinSupervisorBackend()
    monkeypatch.setattr(b, "_schtasks", lambda args: 0)
    b.install(r"C:\py\python.exe", str(tmp_path / "app"), plugin_root="/plug")
    assert calls[:2] == [("hooks", r"C:\py\pythonw.exe", "/plug"), ("task",)]
    b.uninstall()
    assert calls[-1] == ("unhooks",)
    assert b.hooks_doctor_row() == ("hooks installed", True, "x")
