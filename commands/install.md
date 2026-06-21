---
description: Install Sonara (one-time): speech engine, autostart, hooks, hotkeys
---

Run the Sonara install command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" install
```

This is the one-time setup. It installs the Windows speech engine (PyWinRT) into your
Python, copies the runtime to `~/.sonara/app`, registers the background daemon to
autostart, wires up the Claude Code hooks, and sets up the global hotkeys. It can take a
minute because it downloads the speech-engine packages with pip.

Print the command's output to the user verbatim so they can see each step. If it warns
that the speech engine could not be installed, relay the manual `pip install` command it
printed. When it finishes, tell the user to start a new Claude Code session (or run
`/sonara:doctor`) to confirm speech is working.
