---
description: Set how many items Sonari batches before reading (1 = read immediately)
argument-hint: <n>
---

Run the Sonari minqueue command with the Bash tool, forwarding the requested
item count (1-10):

```
bash "${CLAUDE_PLUGIN_ROOT}/bin/sonari" minqueue $ARGUMENTS
```

A value above 1 holds prose until that many items accumulate, then reads them in
one batch (a finished message below the threshold is still read). 1 reads each
item as it arrives.

Print the command's output to the user verbatim. If the command errors (for
example a non-integer value), report the error briefly.
