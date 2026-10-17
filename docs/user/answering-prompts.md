# Answering prompts

This page explains how Sonara reads questions, permission prompts and plans, how you answer them by number, and what each sound means.

## The flow

1. A sound plays the moment a decision needs you, in every session, even one that is not speaking.
2. Sonara reads the decision after the sentences before it, so the context comes first.
3. You answer in Claude Code by pressing the option's number. Sonara does not press keys for you.

A decision is read with priority: once the message being read ends, that session goes ahead of the others.

## What each decision sounds like

| Decision | Sound | What is read |
|---|---|---|
| Question | `choice` (once while a question waits for an answer) | The question, then "Option 1: label. description. Option 2: ..." |
| Permission prompt | `permission` | The pending action: the command, the file name, or the tool name. "Permission needed." if there is nothing to say |
| Plan | None | "Plan ready." and then the plan, or "A plan is ready for your review." |

Notes:

- A multi-select question adds "This is a multi-select; you can pick more than one."
- When a question is waiting, the permission prompt that Claude Code fires with it is dropped, so you do not hear it twice.
- A plan has no sound. It is read in its place in the reply.

## Answering

| You want to | Press |
|---|---|
| Pick an option | Its **number**, 1 to 9, in Claude Code |
| Cancel | `Esc` |
| Pick several (multi-select) | Each option's number, or `Space` on the highlighted item, then `Enter` |
| Pick option 10 or later | The arrow keys, then `Enter` |

Sonara always says the multi-select and the more-than-nine-options notes when they apply. The short hint "Press the option's number to choose, or Escape to cancel." (then, once per session, "Selecting is immediate.") is spoken only when **Detail** is **Everything**.

## Several questions at once

Claude Code shows a set of questions one at a time. Sonara has two ways to read them, set by **Step through questions** on the Speech page (default **Off**).

| Setting | What you hear |
|---|---|
| Off (default) | The reply, then every question in turn: "Question 1 of 4. ...", "Question 2 of 4. ..." |
| On | The reply and only the first question ("Question 1 of 4"). The others wait for you |

With stepping on, the questions are sections after the reply:

| Key | Action |
|---|---|
| Ctrl+Alt+Down (tap) | Next question |
| Ctrl+Alt+Up (tap) | The question before. On question 1, back into the reply |
| Ctrl+Alt+Up (hold half a second) | Read the whole message again: the reply, then question 1 |
| Ctrl+Alt+Down (hold half a second) | Flush: skip the rest of the reply. Questions that arrive are still read |

**Stepping and answering.** With stepping on, answering a question in Claude Code does not read the next one. Claude Code tells plugins nothing until the whole set is answered, so press **Ctrl+Alt+Down** to hear the next question.

**Thinking before a question.** If Claude only thought before asking (Claude Code shows the thinking but sends plugins no text), Sonara reads that last thinking before the question.

## The sounds

Sonara has nine built-in sounds. They play one after the other, never on top of each other. The key sounds (`step`, `nav`, `nav_edge`) are the exception: they play the moment the key acts, over a sound that is already playing.

| Sound | When | What it sounds like |
|---|---|---|
| `choice` | A question needs your answer | Two bubble pops, rising |
| `permission` | A permission prompt | Two soft rising blips |
| `turn_done` | A reply ends | Marimba, two notes up a third |
| `session_change` | The voice moves to another session (then "Session changed: *folder*.") | Slow, warm e-piano arpeggio |
| `nav` | A restart (hold Ctrl+Alt+Up) or a flush (hold Ctrl+Alt+Down) | Short rewind swell into a light tick |
| `step` | A tap on Ctrl+Alt+Up or Down moved one section or code line | The light tick |
| `nav_edge` | A tap had nothing to move to | The light tick alone |
| `error` | Speech failed | Low falling knock pair |
| `summary_failed` | A summary-mode turn had no text to read | Short, soft falling knock |

Muting changes the sounds too: **Muted** silences speech but keeps the sounds, **Super muted** silences both (see [Hotkeys](hotkeys.md)).

### Your own sounds

Put a WAV named after a sound in `%LOCALAPPDATA%\Sonara\earcons\` and Sonara plays it instead of the built-in one.

| Rule | Value |
|---|---|
| File names | `choice.wav`, `permission.wav`, `turn_done.wav`, `session_change.wav`, `nav.wav`, `step.wav`, `nav_edge.wav`, `error.wav`, `summary_failed.wav` |
| Format | 8, 16, 24 or 32-bit integer, or float. Mono or stereo. Any sample rate |
| Length | Up to 10 seconds |
| Size | Up to 16 MiB |
| When it applies | The next time that sound plays. No restart |
| Back to the built-in | Delete the file |

A file Sonara cannot play is skipped, the built-in sound plays, and `logs\sonarad.log` says why. The Audio page, under *Chimes*, shows the folder and which are *Your chimes*.

## See also

- [Reading and code](reading-and-code.md): what else is read, and Detail levels
- [Hotkeys](hotkeys.md): tap and hold on Ctrl+Alt+Up and Down, mute, sessions
- [Sessions](sessions.md): how decisions in other sessions are announced
- [Settings](settings.md): Step through questions and the other settings
- [Troubleshooting](troubleshooting.md): if a prompt was not read
