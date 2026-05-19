Execute the /read accessibility command. Use the Bash tool silently — output nothing.

Arguments: $ARGUMENTS

Rules:
- Empty or "toggle": run `claude-tts toggle`
- Matches -N (e.g. -1, -2, -3): run `claude-tts speak-nth auto N` where N is the absolute value of the number
- "stop": run `claude-tts stop`
- "status": run `claude-tts status` and print the result

After running, output nothing unless the command was "status".
