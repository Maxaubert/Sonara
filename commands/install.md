---
description: Install Sonara (one-time): speech engine, autostart, hooks, hotkeys
---

Run the Sonara install command with the PowerShell tool:

```
powershell -ExecutionPolicy Bypass -File "${CLAUDE_PLUGIN_ROOT}/bin/sonara-bootstrap.ps1"
```

This is the one-time setup. If you have no Python, it first provisions one (a
uv-managed CPython); then it installs the Windows speech engine (PyWinRT), copies the
runtime to `~/.sonara/app`, registers the background daemon to autostart, wires up the
Claude Code hooks, and sets up the global hotkeys. It can take a couple of minutes the
first time (it may download Python + the speech-engine packages).

Print the command's output to the user verbatim so they can see each step. If it warns
that the speech engine could not be installed, relay the manual `pip install` command it
printed. When it finishes, tell the user to start a new Claude Code session (or run
`/sonara:doctor`) to confirm speech is working.
