---
description: Start the Sonara speech daemon (clears a previous shut down)
---

Run the Sonara start command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" start
```

This launches the background speech daemon. Use it to bring Sonara back after a
"Shut down" from the settings page: shutting down writes a stop sentinel that
keeps the daemon down (the session hooks will not auto-respawn it), and this
command clears that sentinel and starts the daemon again. It is the counterpart
to shutting down from settings, which cannot host a start control because the
settings page is served by the daemon itself.

Print the command's output to the user verbatim. If it reports the daemon is
already running, tell the user that. If the command errors, report the error
briefly.
