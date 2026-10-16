# Following the question on screen (spec and plan, 2026-10-16)

Status: open plan, tracked in issue #323 (not yet implemented). Research: the widget probe
(`research/sonara/2026-10-16-question-widget-probe.md` in the maintainer's notes, not in this repo).

## Problem

Claude Code shows the questions of one AskUserQuestion one at a time and tells plugins nothing
until the whole set is submitted. So when the user answers question 1, or moves between questions
with the arrows or Tab, Sonara does not know, and with `question_step` on (#320) the next question
is never read unless the user presses Ctrl+Alt+Down. Owner: "you'd like to have it be able to
detect when you move between questions both back and forwards", reliably, not by guessing keys.

## What the probe showed

Every session runs in PrismTerminal, which draws the terminal (xterm.js) and can read its buffer.
Claude Code v2.1.301 draws the widget as:

```
←  ☐ Service  ☒ Platforms  ☐ Release name  ✔ Submit  →
                                                        (blank)
│ When the installer finds the search service already running from an
│ earlier version, should it stop the service, ...    (a long question: "│ " lines)
                                                        (blank)
❯ 1. Restart now                                        (or "  1. [ ] Windows" for multi-select)
...
Enter to select · Tab/Arrow keys to navigate · Esc to cancel
```

- The text between the tab row and the first option is the question showing, in full. It changes
  on Right/Left/Tab/Shift+Tab and when an answer moves on; it does not change while the user types
  under "Type something" (the arrows move the text cursor there).
- `☒` marks an answered tab. After the last question the screen reads "Review your answers".
- Holds at 120 and 70 columns, single and multi-select, one-line and wrapped questions.

## Design

**PrismTerminal (`core/`, ships to both apps):** a question watcher per tab. After the tab's buffer
changes (debounced about 100 ms) it looks at the visible rows for the widget: a row that starts
with `←` and ends with `✔ Submit  →`, followed later by a row that starts with `Enter to select`.
It extracts the question showing: the rows after the tab row up to the first option row (`❯ 1.`
or `  1.`), with a leading `│ ` removed, joined with single spaces. On "Review your answers" it
extracts the marker `review`. When the extract differs from the last one it sent for that tab, the
renderer tells main, and main sends it to Sonara. Prism needs the tab's Claude session id (its own
Claude hooks already report sessions to the indicator; to verify in step 1) and Sonara's
`runtime.json` (`%LOCALAPPDATA%\Sonara`, or `SONARA_HOME`): no Sonara, nothing is sent and nothing
fails. Localhost only; PRIVACY.md says so.

**Sonara:** a new `agent` message `question_shown` (protocol 1.12): `channel`, and `text` (the
question as shown) or `review: true`. The runtime matches `text` against the session's question
set (case and whitespace folded; the shown text is the question Claude sent, so an exact match
after folding, with a prefix match as the fallback for a cut-off screen).
- With `question_step` on: a match that is not the current question becomes the current one and
  is read now, as Ctrl+Alt+Down/Up would (cutting what that session is reading, never another
  session's text). The current question is ignored, so the widget appearing while the message
  text is read does not cut the text.
- With `question_step` off (every question read in order): the set is followed from the first
  `question_shown` for it, and only the question on screen is read (owner decision below).
- `review: true`: says "Review your answers." once per set (a short cue, no options).
- No set, no match, an unknown channel: nothing happens (logged once, `question: shown text not in
  the set`), reply `ok` with `moved: false`.
- Reply `{ok, moved, question, of}` like `next_question`.

## Owner decision: only the question on screen (2026-10-16)

Owner (Ove), 2026-10-16: "it follows the question, never reading a new one that's not on screen.
Going to a different question while one is reading makes that one stop and then the new one read."
Before this, with `question_step` off a reported question was read from its entry to the end of
the set, so questions not on screen were read and heard ones read again. Now, once a host has sent
`question_shown` for a set read in order:

- Only the question on screen is read: Sonara stops at the end of its text and does not go on to
  the next question. The other questions' entries are set aside in L2 (`Router::set_aside`: kept
  in the batch, never fed, not unread), so a replay (Up) reads the message with the question on
  screen only. "Review your answers." is read only when the screen shows the review.
- A different question on screen stops the one being read at once and reads the new one, heard
  before or not. The same question again (a redraw) does nothing. A question of the set read in
  order before the screen's first report that is not on screen stops for the one on screen.
- No `question_shown` for the set (a plain terminal): unchanged, every question in order (#322).
- Muted: the set still follows the screen, nothing is read (the earlier mute fix stays).
- Tests: `crates/sonara-agent/tests/agent.rs` (`a_shown_question_is_read_alone_not_the_rest_of_the_set`,
  `moving_to_another_question_stops_the_one_reading` and others), `tests/rules.rs`, the router's
  unit test, `conformance/agent/test_agent.py::test_question_shown_reads_only_the_question_on_screen`.

## Plan

### Sonara (one PR, version 0.33.0, protocol 1.12)

1. Rules: `Rules::show_question(channel, text)` returns the landing (index) or none; text folding
   and matching; tests with the probe's texts (one-line, wrapped, multi-select).
2. Agent: `Agent::question_shown(channel, text | review)`: with stepping, `land_on` plus
   `read_from` (as `question`); without stepping, `read_from` the question's entry (the entries of
   a read-in-order set are recorded per question); review cue. Agent tests: forward, back, answer
   moving on, the current question ignored, review once, another session's reading untouched.
3. sonarad: the message in `agent_ext` (`TYPES`), reply fields, a log line `question shown
   session=<label> question=<k>/<n>`; conformance tests; `docs/protocol-v1.md` (message, 1.12);
   both SDKs (`question_shown` in TS `AgentApi` and Python `extensions.py`) with namespace tests.
4. README: questions follow the screen in PrismTerminal.

### PrismTerminal (one PR, core version bump; ships to Prism too)

1. Probe the session id: where Prism's Claude hooks give the tab its session id; a unit test.
2. `core/renderer`: the widget extractor as a pure function over buffer rows, unit-tested on the
   probe's screens (saved as fixtures: 120 and 70 columns, wrapped, multi-select, typing, review);
   the watcher (debounced, only Claude tabs, only on change).
3. `core/main`: read `runtime.json`, POST `question_shown` with the token; ignore every failure
   quietly (a diag crumb only). PRIVACY.md: the localhost request.
4. e2e: a stand-in shell prints the widget; a fake Sonara (a local HTTP listener in the scenario)
   records the posts: one per change, none for typing.

### Order and checks

Sonara first (the message can be tested with curl), then Prism. Then a live check on this PC: a
three-question set, answer and move around, Sonara follows. Each PR gets the usual gates, CI and
the owner's merge question.

## Not in scope

Other terminals (Windows Terminal, VS Code): they do not expose the screen to Sonara. Permission
prompts and plans (single screens, already read). Reading the options of a moved-to question twice.
