---
description: Uninstall Sonara (removes autostart, launcher, and ~/.sonara/app; keeps settings)
---

Run the Sonara uninstall command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" uninstall
```

This removes the autostart entry, the launcher, and `~/.sonara/app`. It keeps your
settings (`config.json`, `keymap.json`) so a later reinstall restores your preferences.

Print the command's output to the user verbatim. Do not add commentary beyond the raw
output.
