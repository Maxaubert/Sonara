---
description: Set how far Audio Control lowers other apps' audio (0-100 percent)
argument-hint: <0-100>
---

Run the Sonara duck-level command with the Bash tool, forwarding the percent value:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" duck-level $ARGUMENTS
```

Print the command's output to the user verbatim. If the command errors (for example
a value outside 0-100), report the error briefly.
