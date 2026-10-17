# Spoken summaries

Summary mode makes Sonara speak a short recap of each finished reply instead of reading the whole text.

## At a glance

| | |
|---|---|
| Default | Off |
| Where | Settings page, **Summary** page (`/sonara:settings`) |
| Modes | Off, Tidy, Natural, Brief |
| Who writes the recap | Claude (Haiku) or Codex, on your own login |
| Cost | One small call per finished reply, on that account |
| If it fails | Sonara reads the original reply |

## What changes when it is on

- Sonara waits until the reply is finished, then speaks the recap.
- Questions, plans and permission prompts are still read in full.
- Every chime still plays.
- A new prompt drops the previous reply, as always.
- Hold **Ctrl+Alt+Up** (half a second) to hear the last summary again.
- The **Live reading** controls (Immediately, Queue, When done) do not apply while summaries are on. They come back when you switch to Off.

## The three styles

| Style | What you hear | Good for |
|---|---|---|
| **Tidy** | The whole message, rewritten to be spoken. Nothing left out, same order, about the same length. Code blocks become a one-phrase description. | Hearing everything, minus the symbols |
| **Natural** (default style) | A shorter version. Keeps decisions, results, questions and anything you must act on. Drops process narration, file paths and line numbers. | Daily use |
| **Brief** | One to three short sentences: the outcome, any decision, and what you must do. A question Claude asked always survives. | Long sessions, quick checks |

All three speak as the assistant ("I fixed the bug") and keep every item of a list. You can replace the instruction for a style: edit the **Prompt** box on the Summary page. **Reset** puts the built-in one back.

## Who writes the recap

You pick a model on the Summary page. The model decides which tool runs.

| Model in the list | Runs | Needs |
|---|---|---|
| **Haiku (fast)**, the default | `claude -p` with tools and settings turned off | Claude Code, already logged in |
| **GPT-5.4 Mini (faster)** | `codex exec` with its tools turned off | Codex CLI on your PATH, logged in |
| **GPT-5.6 Luna (fastest)** | `codex exec` with its tools turned off | Codex CLI on your PATH, logged in |

The call is separate and throwaway. It never touches your Claude Code session. With settings turned off, no plugin runs inside it, Sonara's own hooks included.

## Cost and privacy

- **Cost.** Each finished reply is one small call on your Claude or Codex account. A long session adds up. Turn summaries off, or pick a smaller model, to spend less.
- **Privacy.** The text of the finished reply leaves your PC: to Anthropic with Claude, to OpenAI with Codex. Nothing is sent while summaries are Off. Details: [PRIVACY.md](../../PRIVACY.md).

## Settings

| Label | Range | Default |
|---|---|---|
| Summary | Off, Tidy, Natural, Brief | Off (style Natural once on) |
| Model | the list above | Haiku |
| Summary timeout | 15 to 300 s | 60 s |
| Settle time | 0 to 5000 ms | 600 ms |

Settle time is the quiet time after a reply ends before Sonara asks for the recap. Timeout is how long it waits for the recap itself.

## When a summary fails

| What happened | What you get |
|---|---|
| The call fails, times out or returns nothing | Sonara reads the original reply instead |
| You send a new prompt or stop before the recap arrives | The recap is dropped |
| The turn had no text at all | The `summary_failed` chime (a short, soft falling knock) and nothing is read |

If the recap never works, check that `claude` (or `codex`) runs in a normal terminal on this PC and that you are logged in. The reason is in `logs\sonarad.log`; see [Troubleshooting](troubleshooting.md#logs). The `summaries` row of `/sonara:doctor` shows whether summaries are on, and with which style and model.

## See also

- [Settings](settings.md): every setting and its default
- [Reading and code](reading-and-code.md): how a full reply is read
- [Hotkeys](hotkeys.md): hold Up to hear the last summary again
- [Troubleshooting](troubleshooting.md)
- [PRIVACY.md](../../PRIVACY.md): what is sent
