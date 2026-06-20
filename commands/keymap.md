---
description: List Sonara hotkey bindings (incl. unbound); '<action> clear' to unbind
argument-hint: [<action> clear]
---

Run the Sonara keymap command with the Bash tool, forwarding any arguments:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" keymap $ARGUMENTS
```

Print the command's output to the user verbatim. With no arguments it lists every
action and its hotkey (unbound actions show "(unbound)"). `<action> clear` (or
`<action> none`) unbinds that action and applies it live (the daemon re-registers
its global hotkeys; on macOS the separate hotkeyd is reloaded to re-read the
keymap).
