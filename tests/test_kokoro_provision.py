import os
import sys
import pytest
from sonara import paths, kokoro_provision as kp


def test_kokoro_venv_python_path_is_platform_correct(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "KOKORO_VENV", tmp_path / "venv")
    p = paths.kokoro_venv_python()
    if sys.platform == "win32":
        assert p.endswith(os.path.join("venv", "Scripts", "python.exe"))
    else:
        assert p.endswith(os.path.join("venv", "bin", "python"))


def test_neural_enabled_reflects_venv_python_existence(monkeypatch, tmp_path):
    venv = tmp_path / "venv"
    monkeypatch.setattr(paths, "KOKORO_VENV", venv)
    assert kp.neural_enabled() is False
    # Create the venv python file.
    pybin = tmp_path / "venv" / ("Scripts" if sys.platform == "win32" else "bin")
    pybin.mkdir(parents=True)
    (pybin / ("python.exe" if sys.platform == "win32" else "python")).write_text("")
    assert kp.neural_enabled() is True


# ---------------------------------------------------------------------------
# Task 3: ensure_uv
# ---------------------------------------------------------------------------

def test_ensure_uv_returns_path_when_already_present():
    got = kp.ensure_uv(which=lambda name: "C:/tools/uv.exe",
                       run=lambda *a, **k: pytest.fail("must not bootstrap"))
    assert got == "C:/tools/uv.exe"


def test_ensure_uv_bootstraps_via_pip_when_absent(tmp_path):
    calls = []
    scripts_dir = tmp_path
    (scripts_dir / "uv.exe").write_text("")  # pip install lands uv.exe in the Scripts dir

    def fake_run(cmd, **k):
        calls.append(cmd)

    got = kp.ensure_uv(
        which=lambda name: None,                     # not on PATH
        run=fake_run,
        base_python="python.exe",
        user_scripts=lambda py: str(scripts_dir),
    )
    assert got == str(scripts_dir / "uv.exe")
    assert any("pip" in c and "uv" in c for c in calls)  # bootstrap ran


def test_ensure_uv_raises_actionable_when_unfindable(tmp_path):
    with pytest.raises(RuntimeError) as ei:
        kp.ensure_uv(which=lambda name: None, run=lambda *a, **k: None,
                     base_python="/usr/bin/python3",
                     user_scripts=lambda py: str(tmp_path))  # no uv ever appears
    assert "uv" in str(ei.value).lower()


def test_ensure_uv_windows_uses_scripts_uv_exe(monkeypatch, tmp_path):
    monkeypatch.setattr(kp.sys, "platform", "win32")
    (tmp_path / "uv.exe").write_text("")
    got = kp.ensure_uv(
        which=lambda n: None,
        run=lambda *a, **k: None,
        base_python="py",
        user_scripts=lambda py: str(tmp_path),
    )
    assert got == str(tmp_path / "uv.exe")


# ---------------------------------------------------------------------------
# Task 4: requirements_path + provision
# ---------------------------------------------------------------------------

def test_requirements_file_pins_verified_versions():
    text = open(kp.requirements_path()).read()
    assert "kokoro-onnx==0.5.0" in text
    assert "onnxruntime==1.27.0" in text
    assert "numpy==2.4.6" in text


def test_provision_runs_uv_venv_then_pip_install(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "KOKORO_VENV", tmp_path / "venv")
    monkeypatch.setattr(paths, "kokoro_venv_python",
                        lambda: str(tmp_path / "venv" / "bin" / "python"))
    cmds = []
    kp.provision("/bin/uv", run=lambda cmd, **k: cmds.append(cmd))
    assert cmds[0] == ["/bin/uv", "venv", str(tmp_path / "venv"), "--python", "3.12"]
    assert cmds[1][:4] == ["/bin/uv", "pip", "install", "--python"]
    assert "-r" in cmds[1] and kp.requirements_path() in cmds[1]


# ---------------------------------------------------------------------------
# Task 5: predownload_model + neural_healthy
# ---------------------------------------------------------------------------

def test_predownload_invokes_venv_python_with_pythonpath(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    seen = {}
    def fake_run(cmd, env=None, **k):
        seen["cmd"], seen["env"] = cmd, env
    kp.predownload_model("/app", run=fake_run)
    assert seen["cmd"][0] == "/venv/bin/python"
    assert seen["env"]["PYTHONPATH"] == "/app"
    assert "KokoroEngine" in seen["cmd"][-1]   # the -c body builds the engine


def test_neural_healthy_true_when_venv_reports_installed(monkeypatch):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    assert kp.neural_healthy("/app", run=lambda *a, **k: "True\n") is True
    assert kp.neural_healthy("/app", run=lambda *a, **k: "False\n") is False


def test_neural_healthy_false_on_subprocess_error(monkeypatch):
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: "/venv/bin/python")
    def boom(*a, **k): raise OSError("no python")
    assert kp.neural_healthy("/app", run=boom) is False


# ---------------------------------------------------------------------------
# Task 6: install_kokoro + uninstall_kokoro orchestrators
# ---------------------------------------------------------------------------

def test_install_kokoro_runs_steps_in_order():
    order = []
    kp.install_kokoro(
        "/app",
        ensure_uv=lambda **k: order.append("uv") or "/bin/uv",
        provision=lambda uv, **k: order.append(("provision", uv)),
        predownload_model=lambda app, **k: order.append(("model", app)),
    )
    assert order == ["uv", ("provision", "/bin/uv"), ("model", "/app")]


def test_install_kokoro_aborts_if_provision_fails():
    def boom(uv, **k): raise RuntimeError("uv venv failed")
    with pytest.raises(RuntimeError):
        kp.install_kokoro(
            "/app",
            ensure_uv=lambda **k: "/bin/uv",
            provision=boom,
            predownload_model=lambda app, **k: pytest.fail("must not predownload"),
        )


def test_uninstall_kokoro_removes_venv_idempotently(monkeypatch, tmp_path):
    venv = tmp_path / "venv"; venv.mkdir()
    monkeypatch.setattr(paths, "KOKORO_VENV", venv)
    kp.uninstall_kokoro()
    assert not venv.exists()
    kp.uninstall_kokoro()  # second call must not raise
