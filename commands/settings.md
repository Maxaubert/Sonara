---
description: Open the Sonara settings page in the browser
---

Run the Sonara settings command with the Bash tool:

```bash
"${CLAUDE_PLUGIN_ROOT}/bin/sonara" settings
```

It opens the local settings page in the user's default browser and prints the
URL. If it reports the daemon is not running, tell the user to run
`sonara start` first. Do not print the token-bearing URL back to the user
beyond what the command already printed.
