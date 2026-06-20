# Sonara Windows Fork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline) — the rename is one tightly-coupled mechanical change (package move + import rewrite must be atomic), not parallelizable. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fork Sonari into an independent public Windows repo `Maxaubert/sonara` with a full internal rename sonari → sonara, then migrate the live install.

**Architecture:** Push our consolidated per-session-channels branch (full history) to a new repo, retarget repo references, apply a three-case rename across code/bin/plugin/commands/paths/strings/tests in one coherent change gated by the test suite, then install sonara to a fresh `~/.sonara`.

**Tech Stack:** Python 3.9+, pytest, `gh` CLI (authed as Maxaubert), git, Windows.

## Global Constraints

- Repo: **`Maxaubert/sonara`**, **public**, independent (not a gh fork-relationship).
- Rename, three cases: `sonari`→`sonara`, `Sonari`→`Sonara`, `SONARI`→`SONARA`.
- **Repo-ref gotcha:** retarget `nimkimi/sonari` → `Maxaubert/sonara` in user-facing install/marketplace BEFORE the general rename; after, `git grep -i "nimkimi/sonara"` MUST return nothing.
- **Leave un-renamed:** everything under `docs/superpowers/` (historical) — exclude it from all rename passes.
- **Verification gate:** full suite green except the known pre-existing Windows-environmental failures (test_bin_shims, test_bin_sonari, test_daemon_main::test_ensure_running_spawns_detached_when_socket_absent, test_kokoro_provision, test_paths, test_transport, test_win_autostart, test_win_tts). ANY new failure = incomplete rename.
- **Post-rename grep gate:** `git grep -il "sonari"` returns only `docs/superpowers/` files + this plan/spec.
- macOS code is renamed, not deleted. No feature changes.
- Work happens in the worktree `C:/Users/Admin/Documents/Claude/Github/sonari/.claude/worktrees/feat+per-session-channels` on branch `worktree-feat+per-session-channels`, base HEAD `0de4c2c`.

---

### Task 1: Bootstrap the sonara repo

**Files:** none in-tree (repo + remote setup).

- [ ] **Step 1: Create the public repo**

```bash
gh repo create Maxaubert/sonara --public \
  --description "Sonara — eyes-free speech daemon for Claude Code (Windows line; forked from nimkimi/sonari)"
```
Expected: prints the new repo URL `https://github.com/Maxaubert/sonara`.

- [ ] **Step 2: Add the remote and push the consolidated branch as main**

```bash
cd C:/Users/Admin/Documents/Claude/Github/sonari
git remote add sonara https://github.com/Maxaubert/sonara.git
git push sonara worktree-feat+per-session-channels:main
```
Expected: `* [new branch] worktree-feat+per-session-channels -> main`. (Full history; all 35+ commits.)

- [ ] **Step 3: Verify**

```bash
gh repo view Maxaubert/sonara --json name,visibility,defaultBranchRef -q '{name:.name, vis:.visibility, default:.defaultBranchRef.name}'
```
Expected: `{name: sonara, vis: PUBLIC, default: main}` (default branch may need setting to `main` — if not, `gh repo edit Maxaubert/sonara --default-branch main`).

- [ ] **Step 4: Checkpoint** — report the repo URL; do not proceed to the rename until confirmed.

---

### Task 2: Retarget repo references (before the general rename)

**Files:** Modify `README.md`, `plugin.json`, `marketplace.json`, and any other tracked file outside `docs/superpowers/` that contains `nimkimi/sonari`.

- [ ] **Step 1: Find them**

```bash
git grep -l "nimkimi/sonari" -- ':!docs/superpowers'
```

- [ ] **Step 2: Retarget user-facing repo refs to Maxaubert/sonara**

For each install/marketplace/clone reference that means "this project's repo", replace `nimkimi/sonari` → `Maxaubert/sonara`. Keep ONE explicit upstream-credit line (e.g. in README: "Forked from [nimkimi/sonari](https://github.com/nimkimi/sonari).") verbatim. Do this with targeted edits, not a blind sed, so the credit line survives.

- [ ] **Step 3: Verify no broken org/repo pair will be created later**

```bash
git grep -i "nimkimi/sonari" -- ':!docs/superpowers'   # only the credit line(s) should remain
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: retarget repo references to Maxaubert/sonara (keep upstream credit)"
```

---

### Task 3: Rename the package + all imports (atomic)

**Files:** `git mv src/sonari src/sonara`; rewrite every `import sonari` / `from sonari` / `sonari.` reference across `src/` and `tests/` and `bin/`.

- [ ] **Step 1: Move the package directory**

```bash
git mv src/sonari src/sonara
```

- [ ] **Step 2: Rewrite imports + module refs (lowercase) across code + tests + bin**

Rewrite the token `sonari` → `sonara` in all Python and bin files EXCEPT `docs/superpowers/`. Use a scoped replacement:

```bash
files=$(git grep -Il "sonari" -- 'src/**' 'tests/**' 'bin/**' 'pyproject.toml' 'conftest.py' 2>/dev/null)
for f in $files; do
  python - "$f" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding="utf-8").read()
s=s.replace("sonari","sonara")   # lowercase package/paths/imports
open(p,"w",encoding="utf-8",newline="").write(s)
PY
done
```
(`.sonari` dir, `sonari-hook`, `sonari-daemon`, etc. are all lowercase and covered by this.)

- [ ] **Step 3: Confirm the package imports**

```bash
cd C:/Users/Admin/Documents/Claude/Github/sonari
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -c "import sonara.daemon, sonara.router, sonara.channel; print('sonara imports OK')"
```
Expected: `sonara imports OK`. If a `ModuleNotFoundError: sonari` appears, a reference was missed — grep `git grep -n "sonari" -- src tests bin` and fix.

- [ ] **Step 4: Run the suite**

```bash
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest -q
```
Expected: only the known pre-existing environmental failures fail. Fix any NEW failure (it is a missed rename) before committing.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename the sonari package to sonara (dir + imports + tests + bin)"
```

---

### Task 4: Rename the capitalized + uppercase identifiers + bin filenames

**Files:** bin script filenames; `Sonari`/`SONARI` string + identifier occurrences in `src/`, `tests/`, `bin/`, `pyproject.toml`, `plugin.json`, `marketplace.json`, `commands/`, `README.md`, `PRIVACY.md`, `.gitignore`.

- [ ] **Step 1: Rename the bin script files**

```bash
cd C:/Users/Admin/Documents/Claude/Github/sonari
git mv bin/sonari bin/sonara
git mv bin/sonari-daemon bin/sonara-daemon
git mv bin/sonari-hook bin/sonara-hook
git mv bin/sonari-hook.cmd bin/sonara-hook.cmd
git mv bin/sonari.cmd bin/sonara.cmd
```

- [ ] **Step 2: Rewrite Sonari→Sonara and SONARI→SONARA (and any remaining lowercase outside docs)**

```bash
files=$(git grep -Il -e "Sonari" -e "SONARI" -e "sonari" -- ':!docs/superpowers' 2>/dev/null)
for f in $files; do
  python - "$f" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding="utf-8").read()
s=s.replace("SONARI","SONARA").replace("Sonari","Sonara").replace("sonari","sonara")
open(p,"w",encoding="utf-8",newline="").write(s)
PY
done
```
This covers `SONARI_DIR`, `SONARI_DISABLE_HOTKEYS`, `Sonari.Speechd`, plugin display names, the spoken setup cue ("run /sonara:install"), commands, README, PRIVACY, .gitignore.

- [ ] **Step 3: Fix any repo-ref collateral**

```bash
# the general pass above may have turned a kept "nimkimi/sonari" credit into "nimkimi/sonara" — restore it
git grep -n "nimkimi/sonara" -- ':!docs/superpowers'
```
If found, edit those back to `nimkimi/sonari` (upstream credit) by hand.

- [ ] **Step 4: Verify imports + suite + grep gates**

```bash
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -c "import sonara.daemon; print('OK')"
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m pytest -q     # only known env failures
git grep -il "sonari" -- ':!docs/superpowers'                          # expect: only this plan + the spec
git grep -i "nimkimi/sonara" -- ':!docs/superpowers'                   # expect: nothing
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename Sonari/SONARI identifiers + bin filenames to Sonara/SONARA"
```

---

### Task 5: Verify the plugin + commands manifest are coherent

**Files:** `plugin.json`, `marketplace.json`, `commands/*.md`, `hooks/hooks.json`.

- [ ] **Step 1: Inspect the manifests**

```bash
cat plugin.json marketplace.json | python -m json.tool   # valid JSON, name == "sonara"
git grep -n "sonari\|Sonari\|SONARI" -- plugin.json marketplace.json hooks/hooks.json commands/   # expect nothing
```

- [ ] **Step 2: Confirm hook + command paths point at bin/sonara-hook**

```bash
git grep -n "sonara-hook\|/bin/sonara" -- hooks/hooks.json
```
Expected: hook commands reference `${CLAUDE_PLUGIN_ROOT}/bin/sonara-hook`.

- [ ] **Step 3: Commit any fixes**

```bash
git add -A && git commit -m "chore: coherent sonara plugin + commands + hooks manifest" || echo "(nothing to fix)"
```

- [ ] **Step 4: Push the renamed tree to sonara/main**

```bash
git push sonara HEAD:main
```

- [ ] **Step 5: Checkpoint** — report grep-gate results; pause before the live migration.

---

### Task 6: Live migration to ~/.sonara

**Files:** none in-tree (runtime).

- [ ] **Step 1: Seed ~/.sonara from the existing config**

```bash
mkdir -p ~/.sonara/earcons
cp ~/.sonari/config.json ~/.sonara/config.json
cp ~/.sonari/keymap.json ~/.sonara/keymap.json
cp ~/.sonari/earcons/*.wav ~/.sonara/earcons/ 2>/dev/null || true
# repoint earcon paths in the copied config from \.sonari\earcons to \.sonara\earcons
python - <<'PY'
import json, os
p=os.path.expanduser("~/.sonara/config.json"); c=json.load(open(p))
e=c.get("earcons",{})
for k,v in list(e.items()):
    e[k]=v.replace("\\.sonari\\","\\.sonara\\").replace("/.sonari/","/.sonara/")
json.dump(c,open(p,"w"),indent=2); print("repointed earcons:",{k:v.split("\\")[-1] for k,v in e.items()})
PY
```

- [ ] **Step 2: Retire the old sonari daemon + task + launcher**

```bash
# stop the running sonari daemon
OLD=$(python -c "import json;print(json.load(open(r'C:/Users/Admin/.sonari/daemon.lock'))['pid'])" 2>/dev/null); taskkill //PID "$OLD" //F 2>&1 | tail -1
# remove the old Task Scheduler task so it can't respawn the old daemon
cmd //c "schtasks /delete /tn Sonari.Speechd /f" 2>&1 | tail -1
rm -f ~/.local/bin/sonari ~/.local/bin/sonari.cmd 2>/dev/null || true
```

- [ ] **Step 3: Install sonara from the renamed tree**

```bash
cd C:/Users/Admin/Documents/Claude/Github/sonari
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m sonara.cli install
```
Expected: "Copied runtime to: C:\\Users\\Admin\\.sonara\\app", Task Scheduler task `Sonara.Speechd` registered.

- [ ] **Step 4: Restart + verify**

```bash
# kill the just-started daemon to force a clean respawn on the new task, then poll
OLD=$(python -c "import json;print(json.load(open(r'C:/Users/Admin/.sonara/daemon.lock'))['pid'])" 2>/dev/null); taskkill //PID "$OLD" //F 2>&1 | tail -1
# poll for respawn (60s)
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m sonara.cli status
PYTHONPATH=src "C:/Program Files/Python314/python.exe" -m sonara.cli doctor 2>&1 | grep -i hook
```
Expected: status shows voice=af_heart; doctor hooks row OK; the daemon speaks on the next assistant message. The Claude Code plugin must be re-pointed at the sonara marketplace/dir (the `sonara@sonara` plugin) for hooks to fire — note this for the user.

- [ ] **Step 5: Checkpoint** — confirm sonara speaks + a hotkey works. Leave `~/.sonari` + its (now-deleted) task as rollback; only remove `~/.sonari` after sonara is confirmed.

---

## Self-review notes

- Spec §1 (lineage) → Task 1. §2 rename + repo-ref gotcha → Tasks 2-4 (repo-ref first, then 3-case rename). §3 verification → the import/suite/grep gates in Tasks 3-5. §4 migration → Task 6. §5 phasing → task order. §6 out-of-scope (mac kept, docs left) → enforced by the `':!docs/superpowers'` exclusion and no-delete.
- The rename is deliberately split: Task 3 (lowercase package/imports — the part that breaks compilation if incomplete, gated by an import check) then Task 4 (Sonari/SONARI strings + bin filenames). This keeps each commit's blast radius reviewable and the import-check catches the dangerous case early.
- No new test code — the existing suite is the regression gate; new failures localize missed renames.
- The plugin re-point in Claude Code (enabling `sonara@sonara`) is a user action; flagged in Task 6 Step 4.
