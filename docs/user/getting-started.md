# Getting started

Install Sonara, hear your first reply and learn what to expect in the first five minutes, then how to update and uninstall it.

## Requirements

| Need | Detail |
|---|---|
| Windows | 10 or 11, x64. Sonara is Windows only |
| Claude Code | With plugin support. On Windows it needs Git for Windows, and Sonara's hooks run through its Git Bash |
| Internet | Only on first use, for two one-time downloads (below) |
| Not needed | Python, an installer, administrator rights |

## Install

In Claude Code:

```
/plugin marketplace add Maxaubert/Sonara
/plugin install sonara@sonara
```

Then **restart Claude Code**. There is no setup step.

## What happens on first use

The first hook of the new session sees that the runtime is missing and installs it in the background. Claude Code is not held up.

| Step | What | Where |
|---|---|---|
| 1 | Downloads `sonara-runtime-win-x64-<version>.zip` (about 15 MB) and `SHA256SUMS` from the matching [GitHub release](https://github.com/Maxaubert/Sonara/releases), checks the zip's SHA-256, unpacks it | `%LOCALAPPDATA%\Sonara\runtime\<version>\` |
| 2 | Starts the runtime (`sonarad.exe`). It arms the hotkeys, then downloads the Kokoro voice model (about 350 MB, once, checked against pinned hashes) | `%LOCALAPPDATA%\Sonara\models\` |

Until the model is ready, a Windows voice reads. The voice switches to Kokoro by itself when the download ends.

**The first prompt after installing may stay silent.** The first hook only starts the install and returns at once, so the next event is the first one that speaks, usually a few seconds later.

Offline or blocked downloads: the hooks stay silent and try again after 5 minutes. Run `/sonara:start` to try at once.

## Your first 5 minutes

| When | Do this | You should |
|---|---|---|
| 0:00 | Restart Claude Code after installing | |
| 0:30 | Send any prompt | Hear nothing yet, or a Windows voice. The runtime is installing |
| 1:00 | Send another prompt | Hear Claude's reply once the turn is done |
| 1:30 | Run `/sonara:doctor` | See one row per check. A `[INFO]` engine row shows the voice download progress, for example `105 of 354 MB` |
| 2:00 | Press **Ctrl+Alt+M** while it reads | Hear "Muted." Press again for "Super muted.", again for "Unmuted." |
| 2:30 | Tap **Ctrl+Alt+Up** or **Ctrl+Alt+Down** while it reads | Go back or on one section, with a short tick |
| 3:00 | When Claude asks a question or a permission, listen for the chime | Hear the chime at once, then the question and its options. Press the option's **number** in Claude Code to answer |
| 4:00 | Run `/sonara:settings` | Your browser opens the settings page. Try the voice preview and the speaking rate |

Hotkeys are covered in [hotkeys.md](hotkeys.md), every setting in [settings.md](settings.md).

## What to expect

| Behaviour | Default |
|---|---|
| Voice | Kokoro `af_sarah` at 250 words per minute |
| When a reply is read | After Claude finishes the turn, not while it writes |
| Code blocks and tool announcements | Skipped (detail *Default*) |
| Questions and permission prompts | Read in full, after the text that explains them, with a chime the moment they appear |
| Several sessions | Every session's latest reply is read, one after another |
| Other apps' audio | Media is paused while Sonara speaks, then resumed |
| Summaries | Off |
| Newest message | Sonara reads the latest message. A new prompt drops the old reply |

Every default is changeable. See [settings.md](settings.md).

## Check that it works

`/sonara:doctor` prints one row per check:

| Status | Meaning |
|---|---|
| `[ OK ]` | Fine |
| `[INFO]` | Worth knowing, for example the voice model still downloading |
| `[WARN]` | Worth fixing |
| `[FAIL]` | Broken install, or the runtime cannot start |

The rows are explained in [troubleshooting.md](troubleshooting.md).

## Start, stop and the command line

| Slash command | What it does |
|---|---|
| `/sonara:start` | Starts Sonara, installs the runtime if needed, and clears a previous stop |
| `/sonara:doctor` | Checks the runtime, the voice engine and its download, voices, hotkeys, audio mode and hooks |
| `/sonara:settings` | Opens the settings page in your browser |
| `/sonara:uninstall` | Removes Sonara, keeping what you choose |

The same commands run from Git Bash as `bash "<plugin folder>/bin/sonara" <command>`. Two more exist there: `stop` (Sonara stays off, and the hooks do not start it, until `start`) and `version`.

## Update

1. Update the plugin: `/plugin update sonara@sonara`.
2. Restart Claude Code.
3. The next hook sees the plugin needs a newer runtime, installs it the same way as the first time, replaces the running runtime and removes the old version folder.

Your settings stay. `/sonara:doctor` shows the running version in its `version` and `runtime` rows. If its `hooks` row warns that the plugin needs a different runtime than the one running, start a new Claude Code session.

## Uninstall

1. Run `/sonara:uninstall`. Claude asks what to keep.
2. Remove the plugin: `/plugin uninstall sonara@sonara`.

**What `--keep` does.** The command behind it is `sonara uninstall --keep <list>`. The list is any of the words below, comma separated, or `none`. With no `--keep` on the command line the default is `settings`.

| Keep word | What stays |
|---|---|
| `settings` | `config.json`, `keymap.json`, `session_prefs.json`, `engines.json`, the `earcons\` folder of your own chimes, and the added engines' keys in Windows Credential Manager |
| `models` | `models\`, the Kokoro voice model (about 350 MB) |
| `logs` | `logs\` |
| `none` | Nothing is kept |

Whatever is not kept is removed: the runtime folder (`%LOCALAPPDATA%\Sonara\runtime`), `runtime.json`, `state\`, and the files above that you did not keep. Files in the folder that Sonara did not make are listed and left in place.

Before removing anything, Sonara stops and restores other apps' audio. If it cannot stop, nothing is removed. Afterwards Sonara stays off: the hooks do nothing until you run `/sonara:start`.

To remove what you kept, delete `%LOCALAPPDATA%\Sonara` yourself. [PRIVACY.md](../../PRIVACY.md) lists every file in it. Setting `SONARA_HOME` moves this folder.

## See also

- [Hotkeys](hotkeys.md)
- [Settings reference](settings.md)
- [Reading and code](reading-and-code.md)
- [Answering prompts](answering-prompts.md)
- [Voices and engines](voices-and-engines.md)
- [Troubleshooting](troubleshooting.md)
- [FAQ](faq.md)
- [Privacy and every file Sonara keeps](../../PRIVACY.md)
