---
description: Show Sonara speech daemon status (verbosity, rate, voice, queue)
---

Run the Sonara status command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" status
```

Print the command's output to the user verbatim so they can see the current
verbosity, rate, voice, foreground session, and queue length. Do not add
commentary beyond the raw status. If the command errors, report the error briefly.
