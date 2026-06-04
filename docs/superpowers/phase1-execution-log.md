# Echo Phase 1 — Execution Log & Resume Guide

**Living document.** Updated at every section gate. Read this first to understand
where we are, how the build is being run, and how to continue or debug.

Related docs:
- Spec: `docs/superpowers/specs/2026-06-04-echo-eyes-free-claude-code-design.md`
- Plan (70 TDD tasks): `docs/superpowers/plans/2026-06-04-echo-phase1-output-pipeline.md`
- Open concerns the final review MUST resolve: `docs/superpowers/phase1-review-followups.md`

---

## ⏯️ Resume from here

- **Branch:** `rebuild-echo` (off `master`). Legacy tool preserved at tag `v0-legacy-pty`.
- **Build env:** venv at `./.venv` (Python 3.13.6, pytest 9.0.3). **Always run Python/pytest
  via `./.venv/bin/python -m pytest`** (no system pytest; only `python3` exists on PATH).
- **Verify current state:** `cd <repo> && ./.venv/bin/python -m pytest -q` (expect all green)
  and `git status --short` (expect clean).

### How the build is executed (subagent-driven, section by section)
A reusable workflow runs ONE plan section at a time. Per task it dispatches, sequentially:
implementer (TDD + commit) → spec-compliance review → code-quality review, each with a fix
loop, then a full-suite green gate. Implementers/reviewers run on Sonnet; failures escalate
to Opus.

- **Section runner script:**
  `~/.claude/projects/-Users-Nima-Hakimi-Projects-private-claude-tts/2d3b1c76-2953-4733-8aea-4ccf17834503/workflows/scripts/echo-execute-section-wf_f17fe07a-01b.js`
- **To run the next section:** edit the `const SECTION = "..."` line near the top of that
  script to the next section's `## ` heading from the plan, then invoke
  `Workflow({scriptPath: <that path>})`. (The script derives the section's task list by
  reading the plan file — do NOT rely on the Workflow `args` field; it is not reliably
  delivered to the script.)
- **After each section:** independently verify (`pytest -q` green, tree clean), then update
  the section table below and append any findings.

### Section status (8 sections)
| # | Section (`## ` heading in plan) | Status | Tasks | Gate |
|---|----------------------------------|--------|-------|------|
| 1 | Project scaffolding, plugin manifest & legacy removal | ✅ done | 11 | 5 passed |
| 2 | protocol.py + config.py | ✅ done | 7 | 31 passed |
| 3 | cleaner.py + assembler.py | ✅ done | 7 | 50 passed |
| 4 | queue.py + speaker.py | ✅ done | 9 | 74 passed |
| 5 | sessions.py + daemon.py + client.py | ✅ done | 10 | 119 passed (1 warn) |
| 6 | Golden payload capture + hooks_entry.py + bin/echo-hook | ✅ done (live capture deferred) | 9 | 150 passed (1 warn) |
| 7 | cli.py + bin/echo + slash commands + install/uninstall/doctor + legacy migration | ✅ done (real system untouched) | 8 | 193 passed (1 warn) |
| 8 | End-to-end integration test + README + final verification | ✅ done | 7 | 206 passed (1 warn) |

**🏁 ALL 8 SECTIONS DONE — Phase 1 implementation complete: 206 tests pass, tree clean,
92 commits on `rebuild-echo`. The e2e ordering proof (`tests/test_e2e_pipeline.py`) passes.**

**NOW (in progress):** the final whole-implementation review — consumes
`phase1-review-followups.md` (resolve `load_config` deep-copy + the test-thread warning),
then `superpowers:finishing-a-development-branch`. THEN (with Nima): real golden-payload
capture (`tests/fixtures/CAPTURE_INSTRUCTIONS.md`) and the eyes-free verification
(`docs/phase1-verification-checklist.md`).

### Known MANUAL steps (need Nima / a real Claude session)
- **Section 6 — golden payload capture:** capturing REAL hook stdin (MessageDisplay,
  AskUserQuestion, ExitPlanMode, permission_prompt, idle_prompt) requires running a live
  Claude session with `ECHO_CAPTURE` set. The automated run uses *representative* fixtures
  (task "seed representative golden payload fixtures") so tests pass; the real capture is a
  post-build validation done with Nima.
- **Section 8 — manual eyes-free verification checklist:** screen-off run with Nima.

---

## 📓 Per-section journal

### Section 1 — Project scaffolding, plugin manifest & legacy removal — ✅
- 11/11 tasks DONE. Gate: **5 passed**. Workflow run `wf_f17fe07a-01b` (task `w7jj4m5uc`).
- Commits `a1d91d2` (remove legacy) … `e5b1c3b`. Created: pyproject.toml, .gitignore,
  conftest.py, src/echo/{__init__,paths}.py, .claude-plugin/plugin.json, hooks/hooks.json,
  bin/{echo,echo-daemon,echo-hook}.
- Verified independently: tree clean, legacy gone, 5 passed.
- Finding: implementer added `*.egg-info/` to .gitignore after `pip install -e .`, then a
  spec-fix reverted it (ended `specPass:false`). Tree is clean. → logged in follow-ups.

### Section 2 — protocol.py + config.py — ✅
- 7/7 tasks DONE. Gate: **31 passed**. Workflow run `wf_c1d8d3c9-ccc` (task `wk8vswgn7`).
- Commits `9f949e2` … `a8f7ef5`. Created: src/echo/{protocol,config}.py + tests.
- Finding (IMPORTANT, see follow-ups): the `load_config` "DEFAULTS copy" task oscillated —
  a deep-copy fix (`d720499`) was **reverted** (`08d2ee6`) to satisfy spec literalism, so
  `load_config` may return a SHALLOW copy (nested `earcons` dict shared with `DEFAULTS`).
  → tracked in `phase1-review-followups.md` for the final review.

### Section 3 — cleaner.py + assembler.py — ✅
- 7/7 tasks DONE. Gate: **50 passed**. Workflow run `wf_8a1a92c4-10a` (task `wa3ptx8gi`).
- Created: `src/echo/{cleaner,assembler}.py` + tests. clean_markdown `9be02c5`; ProseAssembler
  `5eb29d1`…`fe286ee`.
- 5 of 7 tasks ended `specPass:false`, but the assembler was **independently verified correct**
  by the controller (direct probe): cross-delta sentence assembly, partial buffering, index
  dedup, final-flush, and code-fence summarization all behave per spec, and the e2e case
  `"Let me check the files. I will start now."` → `['Let me check the files.','I will start now.']`.
  The flags were the reviewer being pedantic about LEGITIMATE corrections: the plan's expected
  fence count "1-line" was wrong for the test input (real = 2 content lines → "2-line"), plus a
  commit-message deviation. **No real bug; cleared — the final review need not re-litigate §3.**

### Section 4 — queue.py + speaker.py — ✅
- 9/9 tasks DONE. Gate: **74 passed**. Workflow run `wf_571420f1-dcb` (task `wcnfhp5ee`).
- Commits `030786a` … `52b032d`. Created: `src/echo/{queue,speaker}.py` + tests.
- Hardened runner worked: only 1 `specPass:false` (the no-code "full suite + commit" task).
- Controller probe confirmed: `jump_to_decision` skips leading prose to the decision item;
  `Speaker.cancel` terminates only its own `say` child (no system-wide `pkill`) — the core fix
  for the legacy interruption bug.

### Section 5 — sessions.py + daemon.py + client.py — ✅
- 10/10 tasks DONE. Gate: **119 passed (1 warning)**. Workflow run `wf_9a0813ab-97e` (task `wboqwsltz`).
- Commits `45cf9c7` … `a8f2dc3`. Created: `src/echo/{sessions,daemon,client}.py` + tests.
- **Controller probe of the integration heart confirms the plan self-review fix:** decision
  earcons fire exactly once (from the EARCON message; the CHOICE/PLAN/PERMISSION content
  messages do NOT earcon — no doubling); choice text = `"Which approach? Option 1: Refactor.
  Option 2: Rewrite."`; permission text = bare `"Run: pytest -q"`; background-session decisions
  are gated (text not enqueued). This matches §8's e2e expectation exactly.
- 1 `specPass:false` (speak-loop threading test, non-blocking). 1 pytest warning: an unhandled
  thread-exception in `tests/test_client_send.py::test_send_no_reply` (test-helper echo-server
  thread) → logged in follow-ups.

### Section 6 — Golden payload capture + hooks_entry.py + bin/echo-hook — ✅
- 9/9 tasks DONE. Gate: **150 passed (1 warning)**. Workflow run `wf_7fbcd8ea-161` (task `waqhow0od`).
- Created: `src/echo/hooks_entry.py`, full `bin/echo-hook` (ECHO_CAPTURE + dispatch + always
  exit 0), representative `tests/fixtures/*.json` (MessageDisplay, AskUserQuestion, ExitPlanMode,
  Bash, permission_prompt, idle_prompt), and `tests/fixtures/CAPTURE_INSTRUCTIONS.md`.
- Controller probe confirms the PRODUCER side: AskUserQuestion→[earcon,choice],
  ExitPlanMode→[earcon,plan], permission_prompt→[earcon,permission], idle→[earcon], Stop→[earcon],
  UserPromptSubmit→[set_foreground,flush], SessionStart→[set_foreground,session_start], unknown→[].
  With §5's daemon probe, BOTH pipeline sides are now verified end-to-end.
- MANUAL (deferred): real golden-payload capture from a live Claude session — see
  `tests/fixtures/CAPTURE_INSTRUCTIONS.md`; to be done with Nima as post-build validation.
- **Incident + fix:** a fix-agent dropped `CAPTURE_INSTRUCTIONS.md` via a DESTRUCTIVE GIT OP (its
  add-commit `3fb7f7b` was orphaned from HEAD by a reset/checkout), despite the file-deletion
  hardening. Recovered from the dangling commit; runner further hardened to allow ONLY
  `git add`/`git commit`. Functional tree unaffected (150 green; both probes pass).

### Section 7 — cli.py + bin/echo + slash commands + install/uninstall/doctor + legacy migration — ✅
- 8/8 tasks DONE. Gate: **193 passed (1 warning)**. Final run `wf_f09a5c38-60d` (task `w1f6rtjy4`).
  First run `wf_1a2c075d-cbb` CRASHED mid-`doctor()` on a flaky "subagent emitted no structured
  output" infra error.
- Created in `src/echo/cli.py`: control subcmds (status/stop/repeat/skip/rate/verbosity/voice/
  daemon), `doctor`, `install`/`uninstall` (+ `_launchagent_plist`, `_launchctl`,
  `LAUNCH_AGENT_PATH = ~/Library/LaunchAgents/com.echo.speechd.plist`), legacy `_clean_zshrc` +
  `_clean_settings_json` + `_legacy_migrate`, `_register_local`, `main`. Slash commands in
  `commands/`: echo:status, echo:stop, echo:repeat, echo:verbosity, echo:doctor.
- **SAFETY VERIFIED (controller probe): the REAL system was NOT touched** — `~/.zshrc` still has
  `alias claude='claude-speak'`; `~/.claude/settings.json` still has the 3 legacy hooks;
  `~/.local/bin/claude-speak`+`claude-tts` still present; NO LaunchAgent installed. Cleaners/
  install were implemented + unit-tested on temp paths only.
- Recovery: discarded the crash's un-reviewed partial `doctor` work, hardened the runner
  (safeAgent retry + idempotency), re-ran. The idempotent re-run made some trivial churn commits
  re-reviewing tasks 1–3 (harmless; fix loops are capped) before doing doctor/install/uninstall/
  slash. The 4 `specPass:false` are those churn tasks; doctor/install/uninstall/slash specPass:true.

### Section 8 — End-to-end integration test + README + final verification — ✅
- 7/7 tasks DONE. Gate: **206 passed (1 warning)**. Workflow run `wf_27ed3dac-999` (task `wzms6lydf`).
- `tests/test_e2e_pipeline.py` (the producer→consumer ordering PROOF) passes both cases:
  `test_scripted_session_full_ordering` and `test_background_session_is_earcon_only`.
  Also: golden-fixture parse test, plugin/hooks manifest-validity test, full `README.md` (153 lines).
- Incident #3 (recovered): the "Manual VERIFICATION CHECKLIST" task created
  `docs/phase1-verification-checklist.md`, then a fix-agent DELETED it (`cce9113` "remove
  unauthorized checklist file") — the file-deletion misfire recurred on a task's OWN legit
  artifact (rules can't fully stop a reviewer mis-judging intended output). Recovered from
  `cce9113~1` and re-committed. Functional code never affected (tests protect it).

## 🐞 Gotchas / debugging notes (controller-level)
- **Workflow `args` is unreliable** for inline/scriptPath runs — it reached the script as a
  non-object and crashed (`TASKS.length` of undefined). Fix: hardcode `SECTION` in the
  script and derive the task list via an agent that reads the plan file. (First section run
  `wf_84a9b78d` failed this way; `wf_f17fe07a-01b` is the corrected runner.)
- **Plan ordering bug (fixed at controller level):** the scaffold section put venv/pytest
  setup LAST, but earlier TDD tasks run pytest. And tasks use bare `python` (only `python3`
  exists here). Fix: pre-created `./.venv` with pytest before any task, and a GLOBAL RULE in
  the runner tells every implementer to use `./.venv/bin/python` (translating any bare
  `python`/`python3` test command). conftest puts `src/` on `sys.path`, so no editable
  install is required for imports.
- **Spec-review loops can oscillate** on trivial literalism (egg-info gitignore;
  load_config deep-copy). When a task ends `specPass:false`, it is logged to the follow-ups
  file rather than blocking the section; the final review reconciles.
- **Models:** implementers + reviewers run on Sonnet for speed; the runner escalates a
  blocked/red-tests implementer to Opus once.
- **A running section's fix-agents can DELETE controller docs.** While §3 ran, two controller
  docs (committed `f2168de`, `ea4e9cb`) were `git rm`'d by §3 fix-agents (`186a6f2`, `5eb29d1`)
  as "scope creep". Restored from git. FIX: (a) only commit controller docs BETWEEN sections,
  never while a section workflow is running; (b) runner HARDENED — a SCOPE-DISCIPLINE rule
  forbids agents touching files outside their task, and the spec reviewer now evaluates ONLY
  the implementer's own commit diff (`git show <sha>`), never the wider tree.
- **Fix-agents can drop work via DESTRUCTIVE GIT OPS, not just file deletion.** In §6,
  `CAPTURE_INSTRUCTIONS.md` (committed in orphaned `3fb7f7b`) vanished from HEAD because a later
  fix-agent ran a reset/checkout. Recovered from the dangling commit. Runner now FORBIDS all
  destructive git commands (only `git add`/`git commit`). Lesson: trust the TEST SUITE + targeted
  controller probes as the source of truth for per-section integrity — multi-agent git churn can
  be messy even when the functional tree ends up correct.
- **A subagent can finish WITHOUT emitting structured output → crashes the whole section.**
  §7's first run died this way mid-`doctor()`. Fix: a `safeAgent` wrapper now catches the throw
  and retries once on opus, returning null (recorded BLOCKED) instead of aborting the section.
  Also added IDEMPOTENCY so a re-run skips already-done tasks — but the re-run still re-REVIEWS
  done tasks (the skip returns current HEAD, which the reviewer compares to the OLD task spec →
  minor churn commits). Future improvement: skip reviews entirely when a task reports already-done.
- **Path casing was a red herring.** Repo's real name is `Projects` (capital); FS is
  case-insensitive, so lowercase paths resolve fine for Bash AND the file tools. The earlier
  "File does not exist" on the execution log was because the file had been DELETED (above), not
  a casing problem.
