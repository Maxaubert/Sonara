---
description: Open the Sonara settings page in the browser
---

Run the Sonara settings command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" settings
```

It installs and starts Sonara's runtime if needed, then opens the local settings page
(voice, speed, summary mode, sessions, audio, hotkeys) in the user's default browser.
Tell the user the page is open. Only when no browser could open it does it print the
address; do not repeat that address: it carries a private token.
