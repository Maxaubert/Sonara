"""Out-of-band turn summarizer: a throwaway `claude -p` call.

The user's main Claude session is NEVER touched. This module spawns a separate,
tool-disabled headless process whose context contains ONLY the piped turn text
plus a fixed instruction, reads one short summary back, and exits. It reuses the
user's existing Claude Code login; a failure of any kind maps to None (the daemon
then plays a brief cue instead of speaking).
"""
from __future__ import annotations

import os
import shutil
import subprocess

INSTRUCTION = (
    "You turn a coding assistant's finished message into a spoken recap. "
    "Reply with ONLY 1-2 short plain sentences capturing the gist. "
    "No markdown, no preamble."
)


def build_argv(command: str, model: str) -> list:
    """The headless summarizer invocation. --tools "" disables every tool, so the
    call is pure text-in/text-out: it cannot read files or run commands.
    --setting-sources "" stops the child loading ANY settings, so plugins (and
    with them Sonara's own hooks) never run inside the summarizer session.
    Without it the child's UserPromptSubmit/Stop hooks steal the foreground and
    make the daemon summarize its own summarizer: an endless chime loop that
    spawns a new claude process every few seconds (verified live)."""
    return [command, "-p", INSTRUCTION, "--model", model, "--tools", "",
            "--setting-sources", ""]


def _default_runner(argv, text: str, timeout):
    """Spawn the real subprocess: text on stdin, neutral cwd (the user home, so a
    project CLAUDE.md is never picked up), no console window on Windows. Resolve
    the command via shutil.which because Windows CreateProcess does not apply
    PATHEXT to a bare name like 'claude' (an npm .cmd shim).

    SONARA_SUMMARIZER=1 marks the child so the hook shim (bin/sonara-hook)
    bails out instantly if hooks ever DO fire inside it - the second, redundant
    layer of the recursion guard described in build_argv."""
    exe = shutil.which(argv[0]) or argv[0]
    env = dict(os.environ)
    env["SONARA_SUMMARIZER"] = "1"
    kwargs = {}
    if os.name == "nt":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    proc = subprocess.run(
        [exe] + list(argv[1:]),
        input=text.encode("utf-8"),
        capture_output=True,
        timeout=timeout,
        cwd=os.path.expanduser("~"),
        env=env,
        **kwargs
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def summarize(text, *, model, command: str = "claude", timeout=20, runner=None):
    """A 1-2 sentence spoken summary of *text*, or None on ANY failure
    (non-zero exit, timeout, empty output, spawn error, empty input)."""
    if not (text or "").strip():
        return None
    run = runner or _default_runner
    try:
        code, out = run(build_argv(command, model), text, timeout)
    except Exception:  # noqa: BLE001 - a summarizer failure must never propagate
        return None
    if code != 0:
        return None
    out = (out or "").strip()
    return out or None
