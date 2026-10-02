---
description: Open the Sonara settings page in the browser
---

Run the Sonara settings command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" settings
```

It installs and starts Sonara's runtime if needed, then opens the local settings page
(voice, speed, summary mode, sessions, audio, hotkeys) in the user's default browser and
prints its address. Tell the user the page is open. Do not repeat the address: it carries
a private token.
