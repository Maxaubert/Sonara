---
description: Install or remove neural voices (Kokoro or Chatterbox)
argument-hint: install|uninstall [kokoro|chatterbox]
---

Run the Sonara voices command with the Bash tool, forwarding the requested
action (`install` or `uninstall`) and optional engine (default: `kokoro`):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" voices $ARGUMENTS
```

`install kokoro` provisions a uv-managed Python environment with the Kokoro neural-voice
engine (a one-time ~316 MB download) and repoints the daemon at it. `install chatterbox`
downloads the Chatterbox Turbo model (~2 GB) for GPU-accelerated synthesis with reference-clip
voices. `uninstall` removes the specified engine and reverts to the system voice (or Kokoro
if Chatterbox is removed). Print the command's output verbatim.
If install succeeded, tell the user to pick a voice with /sonara:voice <name> (a bare /sonara:voice lists the installed voices).
If it errors, report the error briefly.
