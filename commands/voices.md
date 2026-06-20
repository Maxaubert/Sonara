---
description: Install or remove Sonara neural (Kokoro) voices
argument-hint: install | uninstall
---

Run the Sonara voices command with the Bash tool, forwarding the requested
action (`install` or `uninstall`):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" voices $ARGUMENTS
```

`install` provisions a uv-managed Python environment with the Kokoro neural-voice
engine (a one-time ~316 MB download) and repoints the daemon at it; `uninstall`
removes it and reverts to the system voice. Print the command's output verbatim.
If it succeeded, tell the user to pick a neural voice with /sonara:voice af_heart.
If it errors, report the error briefly.
