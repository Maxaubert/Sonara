import os
import shutil
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
        py_env=lambda py: {},                        # a plain system Python
    )
    assert got == str(scripts_dir / "uv.exe")
    assert any("pip" in c and "uv" in c for c in calls)  # bootstrap ran


def test_ensure_uv_raises_actionable_when_unfindable(tmp_path):
    with pytest.raises(RuntimeError) as ei:
        kp.ensure_uv(which=lambda name: None, run=lambda *a, **k: None,
                     base_python="/usr/bin/python3",
                     user_scripts=lambda py: str(tmp_path),  # no uv ever appears
                     py_env=lambda py: {})
    assert "uv" in str(ei.value).lower()


def test_ensure_uv_windows_uses_scripts_uv_exe(monkeypatch, tmp_path):
    monkeypatch.setattr(kp.sys, "platform", "win32")
    (tmp_path / "uv.exe").write_text("")
    got = kp.ensure_uv(
        which=lambda n: None,
        run=lambda *a, **k: None,
        base_python="py",
        user_scripts=lambda py: str(tmp_path),
        py_env=lambda py: {},
    )
    assert got == str(tmp_path / "uv.exe")


def test_ensure_uv_uses_the_bootstrap_uv_in_sonara_tools():
    # E2-uv: the zero-Python bootstrap downloads uv to ~/.sonara/tools and
    # never puts it on PATH; voices install must find it there.
    tools = paths.SONARA_DIR / "tools"
    tools.mkdir(parents=True, exist_ok=True)
    (tools / "uv.exe").write_text("")
    got = kp.ensure_uv(which=lambda name: None,
                       run=lambda *a, **k: pytest.fail("must not pip-install uv"),
                       py_env=lambda py: pytest.fail("must not probe"))
    assert got == str(tools / "uv.exe")


def test_ensure_uv_never_pip_user_installs_on_an_externally_managed_python():
    # A uv-managed (PEP 668) Python refuses `pip install --user` (E1): say
    # what to do instead of running it.
    with pytest.raises(RuntimeError) as ei:
        kp.ensure_uv(which=lambda name: None,
                     run=lambda *a, **k: pytest.fail("must not run pip"),
                     base_python="managed-python.exe",
                     py_env=lambda py: {"managed": True})
    assert "uv" in str(ei.value).lower()


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


def _existing_venv(monkeypatch, tmp_path):
    venv = tmp_path / "venv"
    py = venv / "Scripts" / "python.exe"
    py.parent.mkdir(parents=True)
    py.write_text("")
    monkeypatch.setattr(paths, "KOKORO_VENV", venv)
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: str(py))
    return venv


def test_provision_rebuilds_a_venv_whose_python_cannot_start(monkeypatch, tmp_path):
    # E10: the base interpreter is gone (uv python uninstall, moved
    # %APPDATA%): the python.exe stub stays but cannot start, so reusing the
    # venv failed forever and doctor's advice looped.
    venv = _existing_venv(monkeypatch, tmp_path)
    removed, cmds = [], []

    def rmtree(p):
        removed.append(p)
        shutil.rmtree(p)
    kp.provision("/bin/uv", run=lambda cmd, **k: cmds.append(cmd),
                 starts=lambda py: False, rmtree=rmtree)
    assert removed == [str(venv)]
    assert cmds[0] == ["/bin/uv", "venv", str(venv), "--python", "3.12"]
    assert cmds[1][:3] == ["/bin/uv", "pip", "install"]


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


def test_predownload_retries_even_inside_the_failure_cooldown(monkeypatch):
    """An explicit `voices install` is the user asking to retry now: the
    predownload forces the fetch past a remembered failure (E11)."""
    from sonara import kokoro
    calls = []

    class FakeEngine:
        def __init__(self, model_dir, *a, **k):
            pass

        def download_models(self, force=False):
            calls.append(("download", force))

        def _ensure_loaded(self):
            calls.append(("load",))

    monkeypatch.setattr(kokoro, "KokoroEngine", FakeEngine)
    exec(kp._PREDOWNLOAD, {})
    assert calls == [("download", True), ("load",)]


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


def test_provision_reuses_an_existing_venv(monkeypatch, tmp_path):
    """E10: re-running `voices install` upgrades the packages in place; it
    never recreates (and so first deletes) a venv that already works."""
    py = tmp_path / "venv" / "Scripts" / "python.exe"
    py.parent.mkdir(parents=True)
    py.write_text("")
    monkeypatch.setattr(paths, "KOKORO_VENV", tmp_path / "venv")
    monkeypatch.setattr(paths, "kokoro_venv_python", lambda: str(py))
    cmds = []
    kp.provision("/bin/uv", run=lambda cmd, **k: cmds.append(cmd),
                 starts=lambda p: True,
                 rmtree=lambda p: pytest.fail("must not delete a working venv"))
    assert not any(c[1] == "venv" for c in cmds)
    assert any(c[1:3] == ["pip", "install"] for c in cmds)
