---
description: Toggle Summary mode (speak an AI recap of each finished turn)
argument-hint: [on|off]
---

Run the Sonara summary command with the Bash tool, forwarding any arguments:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" summary $ARGUMENTS
```

Print the command's output to the user verbatim. With no arguments it prints
whether summary mode is on. Note: summary mode sends each finished message to a
separate local `claude -p` call to produce the spoken recap; it is off by default.
