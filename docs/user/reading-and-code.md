# How Sonara reads, and how it reads code

This page explains how Claude's text becomes speech: what is read, how dates, numbers and paths are said, and how code is handled.

## What gets read

| Part of the reply | What Sonara does |
|---|---|
| The latest reply | Read sentence by sentence, in order |
| A question, permission prompt or plan | Read in its natural place, after the text that explains it (see [Answering prompts](answering-prompts.md)) |
| A table | Read row by row (see below) |
| A code block | Depends on **Detail** (see [Code](#code)) |
| Older replies | Not read. There is no stepping back through older replies |

**One message, always the last.** When you send a new prompt, the previous reply is dropped and Sonara follows the new one. Hold **Ctrl+Alt+Up** for half a second to hear the latest reply again from the top.

### When the reply is read

The **Reading** setting (Summary page, under *Live reading*) sets when the words are spoken.

| Value | Meaning |
|---|---|
| Immediately | Each sentence is read as it arrives |
| Queue | Held until enough sentences are waiting (**Queue size**, default 5), or the turn ends |
| When done | Held until the agent has finished the turn. **This is the default** |

A question or permission prompt always releases the text held before it, so the context comes first.

## How things are said

Sonara rewrites text into the words people say before the voice reads it. Dates are read day first.

| You see | Sonara says |
|---|---|
| `12.12.2026` | the twelfth of December twenty twenty six |
| `2026-10-13T14:30:00Z` | the thirteenth of October twenty twenty six at fourteen thirty U T C |
| `14:30`, `09:05`, `2:30pm` | fourteen thirty, nine oh five, two thirty P M |
| `1,024`, `3.14`, `23.5%` | one thousand and twenty four, three point one four, twenty three point five percent |
| `1st`, `21st` | first, twenty first |
| `1/2`, `3/4` | one half, three quarters |
| `$5.99`, `250 kr`, `100 USD` | five dollars and ninety nine cents, two hundred and fifty kroner, one hundred U S dollars |
| `$1-2 million` | one to two million dollars |
| `5 km`, `100 km/h`, `200ms`, `5 GB` | five kilometres, one hundred kilometres per hour, two hundred milliseconds, five gigabytes |
| `v1.20`, `Python 3.12` | version one point twenty, Python three point twelve |
| `12/12 tests`, `Step 1/3` | twelve of twelve tests, Step one of three |
| `API`, `HTTP`, `PNG` | A P I, H T T P, P N G |
| `NASA`, `ASAP` | kept as words |
| `SQL`, `nginx`, `JSON`, `kubectl` | sequel, engine X, jay son, kube control |
| `getUserName`, `max_retry_count` | get user name, max retry count |
| `https://github.com/Maxaubert/Sonara/pull/293` | github dot com, pull two hundred and ninety three |
| `ove@example.com` | ove at example dot com |
| `src/main.rs:42` | src slash main dot R S, line forty two |

A plain whole number under 1000 stays as digits, which every voice reads the same way. Inline code and Markdown link targets are left as they are (inline code is read with the code words below).

### File paths

The **File paths** setting (Speech page) sets how much of a path is read.

| Value | What is read |
|---|---|
| Short (default) | Without the common folders (the drive, `Users\<name>`, `AppData`, `Local`, `Roaming`, `ProgramData`, `Program Files`), at most the last three parts |
| Last 2 | The last two parts |
| Last 3 | The last three parts |
| Full | Every folder |

Example with `C:\Users\Admin\AppData\Local\Sonara\logs\sonarad.log`:

| Value | Said |
|---|---|
| Short | Sonara slash logs slash sonarad dot log |
| Last 2 | logs slash sonarad dot log |
| Last 3 | Sonara slash logs slash sonarad dot log |
| Full | Users slash Admin slash AppData slash Local slash Sonara slash logs slash sonarad dot log |

### Web addresses

A URL is read as the site and the last part of the path: `https://example.com/docs?id=5#top` becomes "example dot com, docs".

### Tables, row by row

Each row of a Markdown table is read with every value after its column name. The header row is not read on its own. Example:

| Fund | Share of funds | Return |
|---|---|---|
| KLP Emerging Markets Index | 16.8% | +32% |

Sonara says: "Fund: K L P Emerging Markets Index. Share of funds: sixteen point eight percent. Return: plus thirty two percent."

**Ctrl+Alt+Up** and **Ctrl+Alt+Down** move one row at a time (see [Hotkeys](hotkeys.md)).

### Your own pronunciations

If Sonara says a word wrong, add it under **Pronunciations** on the Speech page: type the word, type how to say it, and press **Add**. Your words are applied first, before every other rule, and match whole words with the same capitals.

## Code

The **Detail** setting (Speech page) decides what happens with code and tool activity.

| Detail | Code blocks | Tool announcements | "Press the number" hint on questions |
|---|---|---|---|
| **Default** (default) | Skipped silently | No | No |
| **Read code** | Announced, then read line by line | No | No |
| **Everything** | Announced, then read line by line | Yes ("Running ..." or the command or file name) | Yes |

The announcement sounds like "Python code block, 2 lines." A block with no language is "Code block, 1 line."

### How a code block sounds

At **Read code** and **Everything**, each line is read with a number and the symbols are said as words:

> TypeScript code block, 7 lines.
> Line one: function Welcome, empty parens, open brace.
> ...
> End of TypeScript code.

- Symbols are words (dot, comma, open paren, equals, arrow).
- Strings and comments are read as written.
- Names like `getUserName` are read "get user name".
- The brief pauses inside a line are commas.
- Inline code in a sentence, like `useState()`, is read with the same words.

### Code settings

These three rows appear on the Speech page only when Detail is **Read code** or **Everything**.

| Setting | What it does | Range | Default |
|---|---|---|---|
| Code reading speed | Code is read at this percent of your speaking rate | 50 to 100 % | 80 % |
| Line pause | Silence after each code line | 0 to 2 s, in steps of 0.1 s | 0.5 s |
| Step through code | Off: the block is read straight through. On: Sonara waits before each line | Off or On | Off |

### Step through code

With **Step through code** on, Sonara announces the block, says "Use Ctrl+Alt+Down for the next line." (it names whatever key you bound), and waits.

| Key | What happens |
|---|---|
| Ctrl+Alt+Down | Reads the next line, then waits again |
| Ctrl+Alt+Up | Goes to the line before |
| `pause` action (unbound by default; bind it on the Hotkeys page) | Reads the rest of the block straight through |

Past the last line, Sonara goes on with the reply.

With step through off, the same two keys still work while a block is read: Down goes to the next line, Up to the one before.

## See also

- [Answering prompts](answering-prompts.md): questions, permission prompts and plans
- [Hotkeys](hotkeys.md): tap and hold on Ctrl+Alt+Up and Down
- [Settings](settings.md): every setting, range and default
- [Summaries](summaries.md): hear a short recap instead of the whole reply
- [Troubleshooting](troubleshooting.md): if Sonara read the wrong thing or skipped something
