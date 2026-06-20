---
description: Set the Sonara say voice (omit the name to list installed voices)
argument-hint: [voice name]
---

Run the Sonara voice command with the Bash tool, forwarding any requested voice
name (no need to quote multi-word names; omit it entirely to list the installed
voices):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" voice $ARGUMENTS
```

Print the command's output to the user verbatim. If it listed voices, tell the
user to re-run /sonara:voice with one of the names. If it errors, report it briefly.
