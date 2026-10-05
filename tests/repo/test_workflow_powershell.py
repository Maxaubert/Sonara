"""#207: a PowerShell syntax error in release.yml ("$runtimeVersion:" reads as a
scope-qualified variable) only surfaced when the release ran on main, after
the merge. Parse every PowerShell step of the workflows here so CI catches it
in the PR instead."""
from __future__ import annotations

import re
import shutil
import subprocess
from pathlib import Path

import pytest

yaml = pytest.importorskip("yaml")

REPO = Path(__file__).resolve().parents[2]
WORKFLOWS = sorted((REPO / ".github" / "workflows").glob("*.yml"))
PWSH = shutil.which("pwsh")


def _powershell_steps():
    for wf in WORKFLOWS:
        data = yaml.safe_load(wf.read_text(encoding="utf-8"))
        for job_name, job in (data.get("jobs") or {}).items():
            default = ((job.get("defaults") or {}).get("run") or {}).get("shell")
            windows = "windows" in str(job.get("runs-on", ""))
            for i, step in enumerate(job.get("steps") or []):
                if "run" not in step:
                    continue
                shell = step.get("shell", default)
                if shell in ("pwsh", "powershell") or (shell is None and windows):
                    name = "{0}:{1}:{2}".format(wf.name, job_name, step.get("name", i))
                    yield pytest.param(step["run"], id=name)


@pytest.mark.skipif(PWSH is None, reason="pwsh not installed")
@pytest.mark.parametrize("script", list(_powershell_steps()))
def test_powershell_step_parses(script, tmp_path):
    # GitHub expressions are substituted before PowerShell sees the script.
    code = re.sub(r"\$\{\{[^}]*\}\}", "X", script)
    path = tmp_path / "step.ps1"
    path.write_text(code, encoding="utf-8")
    probe = ("$e=$null; $null=[System.Management.Automation.Language.Parser]::ParseFile("
             "'{0}',[ref]$null,[ref]$e); $e | ForEach-Object {{ $_.Message }}").format(path)
    out = subprocess.run([PWSH, "-NoProfile", "-Command", probe],
                         capture_output=True, text=True, timeout=60)
    assert out.stdout.strip() == "", out.stdout
