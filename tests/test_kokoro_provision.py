import os
import sys
from sonari import paths, kokoro_provision as kp


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
