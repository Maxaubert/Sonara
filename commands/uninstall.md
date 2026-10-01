---
description: Uninstall Sonara (stops it; removes autostart, hooks, launcher and ~/.sonara/app; keeps settings)
---

Run the Sonara uninstall command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" uninstall
```

This stops Sonara and removes the autostart task, Sonara's hooks in
`~/.claude/settings.json`, the `sonara` launcher in `~/.local/bin`, `~/.sonara/app`, and
the runtime files in `~/.sonara` (`daemon.lock`, `install.json`, `hotkeys.state.json` and
the logs). Sonara then stays off (a `stopped` marker) until the next install or
`/sonara:start`. It keeps your settings (`config.json`, `keymap.json`), session names,
neural voices (`~/.sonara/venv`, `~/.sonara/kokoro`) and voice clips, and says how to
remove them. If Sonara does not stop, nothing is removed. To silence the hooks
completely, also disable the `sonara` plugin via `/plugin`.

Print the command's output to the user verbatim. Do not add commentary beyond the raw
output.
