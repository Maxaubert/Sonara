---
description: Start Sonara (installs its runtime if needed; clears a previous stop)
---

Run the Sonara start command with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" start
```

It installs Sonara's runtime if it is missing (a one-time download of about 15 MB from
the GitHub release), clears a previous stop (`/sonara:uninstall` and `sonara stop` keep Sonara
off until this command) and starts the speech runtime. A runtime of an older
release is replaced.

Print the command's output to the user verbatim. If the command errors, report the error
briefly.
