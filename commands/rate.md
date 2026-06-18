---
description: Set Sonari speech rate in words per minute
argument-hint: <wpm>
---

Run the Sonari rate command with the Bash tool, forwarding the requested
words-per-minute value:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonari" rate $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors (for
example a value outside 100-400), report the error briefly.
