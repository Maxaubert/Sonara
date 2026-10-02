---
description: Uninstall Sonara (stops it; removes its runtime and files, keeping what you choose)
---

First ask the user, with the AskUserQuestion tool (multiple choice, all optional), what
to keep:

- **Settings**: voice, speed, hotkeys, session names and your own chimes (`settings`)
- **Voice model**: the downloaded Kokoro voice model, about 350 MB (`models`)
- **Logs** (`logs`)

Then run the Sonara uninstall command with the Bash tool, passing the chosen items
comma-separated (or `none` when the user keeps nothing):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" uninstall --keep <items>
```

for example `--keep settings,models`. It stops Sonara (other apps' audio is restored
first), removes its runtime (`%LOCALAPPDATA%\Sonara\runtime`) and the files in its home
(`%LOCALAPPDATA%\Sonara`) except the kept ones, and prints what it removed and kept.
Sonara then stays off: the plugin's hooks do nothing until `/sonara:start`.

Print the command's output to the user verbatim. Then tell the user that the plugin itself
is removed with `/plugin uninstall sonara@sonara`.
