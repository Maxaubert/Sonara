# Frequently asked questions

Short answers to the questions people ask most, with a link to the page that has the detail.

## Questions

**Which systems does it run on?**
Windows 10 and 11, 64-bit, with Claude Code. Nothing else. Claude Code on Windows needs Git for Windows, and Sonara's hooks run through its Git Bash. No Python, no installer and no administrator rights.
More: [Getting started](getting-started.md).

**Does it need the internet?**
Once, to download the runtime (about 15 MB) and the Kokoro voice model (about 350 MB). After that it speaks fully offline. Summaries and cloud voices are the exceptions, and both are off until you turn them on.
More: [Voices and engines](voices-and-engines.md).

**What does it cost?**
Sonara is free (MIT license). Spoken summaries use a small call on your own Claude or Codex account for each reply. A cloud voice is billed by its provider. Nothing else costs anything.
More: [Spoken summaries](summaries.md).

**Does it send my text anywhere?**
Not by default. Text leaves your PC only if you turn on summary mode (to Anthropic or OpenAI) or choose a cloud voice (to that provider). There are no accounts and no telemetry.
More: [PRIVACY.md](../../PRIVACY.md).

**Why does it read only the last message?**
By design: Sonara reads the latest reply, and nothing older gets in the way. Send a new prompt and the previous reply is dropped. Hold **Ctrl+Alt+Up** for half a second to hear the latest reply again from the top.
More: [Reading and code](reading-and-code.md).

**How do I make it stop talking?**
Tap **Ctrl+Alt+Down** to skip a section. Hold it for half a second to flush the rest of the reply. Press **Ctrl+Alt+M** to mute (press again for super mute, which also silences the chimes).
More: [Hotkeys](hotkeys.md).

**How do I answer a question or a permission prompt?**
Press the option's number (1 to 9) in Claude Code, or Esc to cancel. A short sound plays the moment a decision needs you, then Sonara reads the question and its options.
More: [Answering prompts](answering-prompts.md).

**Does it read code aloud?**
Not by default. The default detail level, *Default*, announces a code block and skips it. Choose *Read code* to hear code line by line, or *Everything* to also hear each tool announced.
More: [Reading and code](reading-and-code.md).

**Can I use a different voice?**
Yes. Pick one on the Speech page (there is a preview button). Kokoro offers 28 English voices, and the default is `af_sarah`. You can also use your Windows voices, or add a cloud voice or a speech program of your own.
More: [Voices and engines](voices-and-engines.md).

**It speaks with a Windows voice, not Kokoro. Why?**
The Kokoro model is still downloading or loading. A Windows voice covers the gap and Kokoro takes over when it is ready. `/sonara:doctor` shows the progress in its `engine` row.
More: [Troubleshooting](troubleshooting.md#the-voice-model-download-is-stuck).

**My music pauses when Sonara speaks. Can I stop that?**
Yes. By default Sonara pauses media apps while it speaks and resumes them afterwards. On the Audio page set **Other apps** to **Duck** (lower them) or **Off** (leave them alone).
More: [Settings](settings.md).

**Ctrl+Alt+M types a character on my keyboard. What now?**
On layouts with AltGr (German, Norwegian, Polish and others) Windows treats AltGr as Ctrl+Alt, so the hotkey takes that character away. Rebind the key with Win on the Hotkeys page. `/sonara:doctor` names each clashing key.
More: [Hotkeys](hotkeys.md).

**Can I run several Claude Code sessions?**
Yes. Each session has its own queue and name. Press **Ctrl+Alt+P** to move the voice to the next one. You can mute a session, or set **Other sessions** to **Chime only** so only the current one is read.
More: [Several sessions](sessions.md).

**Where are the settings, and how do I open them?**
Run `/sonara:settings` to open the settings page in your browser. Changes apply at once and are saved in `%LOCALAPPDATA%\Sonara\config.json`, which holds only the values you changed.
More: [Settings](settings.md).

**How do I remove it?**
Run `/sonara:uninstall` (it asks what to keep), then `/plugin uninstall sonara@sonara`. Optionally delete `%LOCALAPPDATA%\Sonara` to remove what you kept.
More: [PRIVACY.md](../../PRIVACY.md) lists every file.

## See also

- [User guide index](README.md)
- [Getting started](getting-started.md)
- [Troubleshooting](troubleshooting.md)
- [Settings](settings.md)
