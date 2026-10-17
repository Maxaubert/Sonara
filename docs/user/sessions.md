# Several sessions

How Sonara handles more than one Claude Code session at once: names, muting, switching and what you hear from the ones in the background.

## The basics

- Each Claude Code session is its own source with its own queue.
- Only one session speaks at a time. The others wait their turn.
- Every session's **latest reply** is read, one after another. Nothing older.
- A question or permission prompt in any session plays its chime at once, even if that session is not the one speaking.

## Names

Sonara names a session after the project it works in, not the folder your shell is in.

| Where the session runs | Name |
|---|---|
| Inside a git repository | The repository's folder name |
| In a Claude Code worktree (`<repo>\.claude\worktrees\<name>`) | The repository, `<repo>` |
| In a linked git worktree | The main repository |
| Outside any repository | The last folder of the path |

The first name a session gets stays, even if you `cd` elsewhere. To change it, type a new name on the **Sessions** page of the settings page (up to 60 characters). Clear the box to go back to the automatic name.

## The Sessions page

Open it with `/sonara:settings`, then **Sessions**.

| Part | What it does |
|---|---|
| **Announce switches** | On (default): the new session's name is spoken when the voice moves. Off: you hear the chime only |
| **Open now** | One row per open session: a name box, its folder and a short id, a tag (**Reading**, **Open**) and an **Audio** switch |
| **Audio** switch | On: you hear this session. Off: this session is held unread (muted) |
| **Earlier (n)** | Closed sessions, kept so their name and Audio choice come back. **Forget** removes one, with its name, Audio choice and anything it left unread |

A muted session is held, not lost. Its chimes still play. Switch Audio back on and its reply is there.

There is no per-session voice. Every session speaks with the voice on the Speech page.

## Other sessions: Read all or Chime only

On the **Speech** page, **Other sessions** decides what happens to sessions that are not the current one.

| Choice | Behaviour |
|---|---|
| **Read all** (default) | Every session's latest reply is read, one after another |
| **Chime only** | Only the current session is read. The others play their chimes and their latest reply waits until you move to them |

## Moving between sessions

| Way | What happens |
|---|---|
| **Ctrl+Alt+P** | The voice moves to the next session |
| Automatic | When the current session has nothing more, the next session with a reply is read |

Rules for the move:

- A reply you had not finished resumes where you left it.
- A reply you had already heard starts over.
- Every move plays the `session_change` chime (a slow, warm e-piano arpeggio), then says "Session changed: *name*." If the reply starts over it says "*name*, reading again." A session with no name gets "Session changed."
- The chime plays once per move, whether you pressed the key or the voice moved on its own.

Rebind the key on the Hotkeys page: [Hotkeys](hotkeys.md).

## Mute, flush and sessions

- **Ctrl+Alt+M** cycles unmuted, muted (speech) and super muted (speech and chimes). It covers all sessions.
- Hold **Ctrl+Alt+Down** to flush. The **Flush skips** setting (Hotkeys page) decides how far it goes:

| Flush skips | Effect |
|---|---|
| **This session** (default) | Skips the session being read, then goes to the next session |
| **Everything queued** | Also drops every other session's finished replies that are waiting |

A session still writing its reply keeps it either way. To silence everything, mute.

## See also

- [Hotkeys](hotkeys.md)
- [Settings](settings.md)
- [Reading and code](reading-and-code.md)
- [Answering prompts](answering-prompts.md)
- [Troubleshooting](troubleshooting.md)
