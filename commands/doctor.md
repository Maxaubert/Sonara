---
description: Check Sonara (runtime, voice engine and model download, voices, hotkeys, audio mode, hooks)
---

Run the Sonara doctor with the Bash tool:

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" doctor
```

It installs Sonara's runtime first if it is missing (a one-time download from GitHub), then
prints one row per check: `[ OK ]`, `[INFO]` (worth knowing, such as the voice model still
downloading), `[WARN]` (worth fixing) or `[FAIL]`.

Print the command's output to the user verbatim so they can see each row. Do not add
commentary beyond the raw output, except one short line on how to fix a `[FAIL]` or
`[WARN]` row when the row itself says how.
