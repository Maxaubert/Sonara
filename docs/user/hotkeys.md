# Hotkeys

Every Sonara hotkey: what a tap and a hold do, which sound confirms it, how to rebind it and what to do when a key clashes.

## Default hotkeys

The default chord is **Ctrl+Alt**. Four actions are bound; the rest ship unbound.

| Hotkey | Tap | Hold 500 ms |
|---|---|---|
| **Ctrl+Alt+Up** | Back one section | Read the latest message again from the top |
| **Ctrl+Alt+Down** | On one section | Flush: skip what is being read |
| **Ctrl+Alt+M** | Mute, or unmute from muted or super muted | Super mute (silences the chimes too) |
| **Ctrl+Alt+P** | Move the voice to the next session (not holdable) | |

A **section** is a sentence, a table row, a code line or a question.

## Tap versus hold

| | |
|---|---|
| Tap | Let go before 500 ms. The action happens when you let go |
| Hold | Keep the key down for 500 ms. The action happens at 500 ms, while the key is still down, so you hear it at once |
| Repeat | Holding does not repeat the action |
| Double press | A second press of mute or pause within 0.3 s is ignored |

Only Up, Down, M (and the two unbound question keys below) tell a tap from a hold. Next session, pause, faster and slower act on press.

## What each action does

| Action | Settings page label | Default | Effect |
|---|---|---|---|
| `restart` | Back / hold: from the start | Ctrl+Alt+Up | Tap: back one section of the reply being read. While code plays, back one code line. In a set of questions, the question before (from question 2 on). With nothing being read, the latest message again. Hold: the latest message again from the top. With a set of questions it reads the reply, then question 1. In summary mode it re-reads the last summary |
| `flush` | Next / hold: flush | Ctrl+Alt+Down | Tap: on one section. After a reply's last sentence come its questions. While code plays, on one code line. Hold: flush, which skips what is playing, what that session has queued and the rest of its reply still arriving. Its questions are still read |
| `mute` | Mute | Ctrl+Alt+M | A tap switches between unmuted and muted; a hold goes to super muted. Any press from super muted unmutes, a second hold included. Muted: text is not spoken, chimes still play. Super muted: chimes are silent too. Says "Muted.", "Super muted." or "Unmuted." |
| `next_channel` | Next session | Ctrl+Alt+P | Moves the voice to the next Claude Code session. Plays the `session_change` chime, then says "Session changed: *folder*." With no session open it says "No session." |
| `pause` | Pause | unbound | Pause or resume. Says "Paused." or "Resumed." (only while something is loaded) |
| `faster` | Faster | unbound | Rate plus 25 words per minute, then says "Rate N." |
| `slower` | Slower | unbound | Rate minus 25 words per minute, then says "Rate N." |
| `previous_question` | Back, second key / hold: from the start | unbound | Same as `restart`: a second key for it |
| `next_question` | Next, second key / hold: skip the rest | unbound | Tap: same as a tap on `flush`. Hold: skip the rest of the message, to question 1 of a set that follows, else a flush of the session being read |

**Flush skips** (Hotkeys page, setting `flush_scope`) decides what a flush drops besides the session being read:

| Choice | Effect |
|---|---|
| This session (default) | Goes on to the next session |
| Everything queued | Also drops every other session's finished replies waiting to be read |

A session still writing its reply keeps it either way. To silence everything, mute with Ctrl+Alt+M. Hold Ctrl+Alt+Up afterwards to hear again what had arrived before the flush.

## Confirmation sounds

Every key press answers with a sound or a spoken word, so you know it worked.

| Chime | Plays when |
|---|---|
| `step` | A tap moved one section, one code line or one question. A short tick |
| `nav` | A hold restarted the message (Up), or a hold flushed something (Down). A short rewind swell into the tick |
| `nav_edge` | Nothing to move to: a tap on the last section, or the edge of a set of questions, or a flush with nothing to drop. The tick on its own |

These three play the moment the key acts, over a chime that is already playing. Other chimes wait their turn. The other six chimes (`choice`, `permission`, `turn_done`, `session_change`, `error`, `summary_failed`) are not key answers. You can replace any chime with your own WAV: see [settings.md](settings.md#chimes-and-where-files-live).

The spoken answers: "Muted.", "Super muted.", "Unmuted.", "Paused.", "Resumed.", "Rate 275.", "No session." and, for a switch, "Session changed: *folder*."

## Hotkeys only work while Sonara is in use

The hotkeys are held only while an app that uses them is connected, such as a Claude Code session with the plugin. With none open, the Hotkeys page says "Hotkeys are off until an app that uses them is connected." and `/sonara:doctor` shows the hotkeys row as `[INFO]` with "(not registered now)".

## Rebind a hotkey

1. Run `/sonara:settings` and open the **Hotkeys** page.
2. Click the key box of the action. It says to press the new shortcut.
3. Press the new chord. **Esc** cancels. The new chord is saved at once.
4. To remove a binding, click the **x** next to it.
5. **Reset all** puts every action back on its default and asks you to confirm.

Rules:

| Rule | Detail |
|---|---|
| Modifier | The chord must include **Ctrl, Alt or Win**. A hotkey without one would take that key away from every app |
| Keys | Letters A to Z, digits 0 to 9, Up, Down, Left, Right, Home, End, Page Up, Page Down, `.`, `[` and `]`. Function keys are not supported |
| Shift | Allowed on top of Ctrl, Alt or Win |
| One chord, one action | If the chord already belongs to another action, the page asks "Move it here?" |
| Where it is saved | `keymap.json` in `%LOCALAPPDATA%\Sonara`, holding only your changes |

Reset all clears every change, including bindings you removed on purpose.

## The Ctrl+Alt and AltGr caveat

Windows sends **AltGr as Ctrl+Alt**. On keyboard layouts with AltGr characters (German, Norwegian, Polish and others), a Ctrl+Alt hotkey takes that character away. Example: Ctrl+Alt+M bound to mute stops AltGr+M from typing µ.

**Fix:** rebind that action with Win added. A chord with Win never counts as AltGr. Resetting to defaults does not help, because the defaults use Ctrl+Alt.

**Why Win+Alt is not the default:** Windows already owns Win+Alt+Up, Win+Alt+Down, Win+Alt+M and Win+Alt+P. Pick another chord, such as Ctrl+Win+*key*, and check the doctor row afterwards.

**Intel graphics:** the driver can own Ctrl+Alt+arrow chords to rotate the screen. Turn its hotkeys off in Intel Graphics Command Center, or rebind the arrow actions with Win.

## How a clash shows up in doctor

Run `/sonara:doctor` and look at the `hotkeys` row.

| Row | Meaning |
|---|---|
| `[ OK ]` with a list such as `Ctrl+Alt+Up restart, Ctrl+Alt+Down flush, ...` | All bound hotkeys are registered |
| `[INFO]` with `(not registered now)` | Nothing is using the hotkeys at the moment |
| `[WARN]` `Ctrl+Alt+M is taken by another program` | Another program owns that chord. Rebind |
| `[WARN]` `Ctrl+Alt+M faster is also bound to mute; rebind one in the settings page` | Two of your actions share one chord (a hand-edited `keymap.json`) |
| `[WARN]` `Ctrl+Alt+M types 'µ' with AltGr on this keyboard; rebind it with Win in the settings page` | The AltGr clash above |
| `[WARN]` Intel graphics line | A taken Ctrl+Alt+arrow. The row adds the Intel Graphics Command Center advice |
| `[WARN]` `keymap.json: ...` | An entry in `keymap.json` names a key or modifier Sonara does not know. That one is skipped, the other hotkeys still work |

The Hotkeys page also marks a chord that another program owns or that two actions share, next to the binding. The AltGr clash is reported only by doctor.

## See also

- [Getting started](getting-started.md)
- [Settings reference](settings.md)
- [Reading and code](reading-and-code.md)
- [Answering prompts](answering-prompts.md)
- [Sessions](sessions.md)
- [Troubleshooting](troubleshooting.md)
