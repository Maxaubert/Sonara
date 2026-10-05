"""Golden cases for the Rust hook adapter (crates/sonara-hook, L5): every
case in crates/sonara-hook/tests/golden/ is a Claude Code hook event (the
captured payloads in tests/fixtures/ and a few inline ones) with the
protocol v1 messages it must become. This test proves the golden messages
are the Python mapping (hooks_entry.handle_event) adapted to the new message
names; crates/sonara-hook/tests/golden.rs proves the Rust mapping produces
them. Together: both mappings agree until the Python plugin is removed (M11).

The adaptation (Python message -> protocol v1):
- PROSE -> stream; EARCON turn_done -> turn_end.
- EARCON choice + CHOICE -> one ask "question" per question (the earcon is
  the ask's own), the TUI notes and selection hints on the last one.
- PLAN -> ask "plan"; EARCON permission + PERMISSION -> ask "permission"
  (text: the action, else the message), both with the selection hints.
- TOOL -> tool; CHOICE_ANSWERED -> answered.
- SET_FOREGROUND -> channel_open (label: the session's project, with
  keep_label, #245; host_tab) + focus;
  FLUSH -> turn_start; SESSION_START adds nothing more (its plugin_version
  and plugin_root fed the Python setup guide, which is not part of L5);
  SESSION_END -> channel_close.
- A missing session id is the channel "default" (protocol v1 needs one).
- Every agent message that names the session (stream, tool, ask, answered,
  turn_end) carries the session's project as "label" (#241; the Python
  plugin sent it only with SET_FOREGROUND).
- The project (#245, crates/sonara-hook/src/project.rs, `_project` here):
  in <repo>/.claude/worktrees/<name> (the first such part) it is <repo>;
  else (not on a UNC path) the repository the
  nearest .git at or above cwd belongs to (a linked worktree's main one),
  never the user's home (USERPROFILE) or above; else the cwd's folder. The Python plugin named the cwd's folder.

Set SONARA_REGEN_GOLDEN=1 to rewrite the expected messages from the Python
mapping (then review the diff)."""
from __future__ import annotations

import json
import ntpath
import os
from pathlib import Path

import pytest

from sonara.daemon import decision_text
from sonara.hooks_entry import handle_event
from sonara.protocol import MsgType

REPO = Path(__file__).resolve().parent.parent
GOLDEN = REPO / "crates" / "sonara-hook" / "tests" / "golden"
FIXTURES = Path(__file__).resolve().parent / "fixtures"
INGEST = REPO / "src" / "sonara" / "daemon" / "ingest.py"

HINT = "Press the option's number to choose, or Escape to cancel."
ONCE = "Selecting is immediate."


def _payload(case):
    if "fixture" in case:
        return json.loads((FIXTURES / case["fixture"]).read_text(encoding="utf-8"))
    return case["payload"]


def _channel(m):
    return m.get("session") or "default"


def _base(kind, m):
    return {"type": kind, "channel": _channel(m)}


def _hinted(d):
    d["hint"] = HINT
    d["hint_once"] = ONCE
    return d


def _option(o):
    if isinstance(o, dict):
        c = {"label": o.get("label", "")}
        if (o.get("description") or "").strip():
            c["description"] = o["description"]
        return c
    return {"label": str(o)}


def _questions(m):
    asks = []
    for q in m.get("questions") or []:
        d = _base("ask", m)
        d["kind"] = "question"
        if isinstance(q, dict):
            d["text"] = q.get("question", "")
            d["options"] = [_option(o) for o in q.get("options", []) or []]
            if q.get("multiSelect"):
                d["multi_select"] = True
        else:
            d["text"] = str(q)
            d["options"] = []
        asks.append(d)
    if not asks:
        asks.append(dict(_base("ask", m), kind="question", text=""))
    last = _hinted(asks[-1])
    notes = decision_text.choice_notes(m)
    if notes:
        last["notes"] = notes
    return asks


LABELLED = ("stream", "tool", "ask", "answered", "turn_end")


def _same_folder(p, home):
    def norm(s):
        return str(s).replace("/", "\\").rstrip("\\").lower()
    return bool(home and home.strip()) and norm(p) == norm(home)


def _small(p):
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            return f.read(4096)
    except OSError:
        return None


def _common_repo(common):
    """project.rs common_repo_name: the folder above .git, or bare x.git's x."""
    name = ntpath.basename(ntpath.normpath(str(common)).rstrip("\\"))
    if name.lower() == ".git":
        return ntpath.basename(ntpath.dirname(ntpath.normpath(str(common)))) or None
    return (name[:-4] if name.endswith(".git") else name) or None


def _linked_repo(worktree, git_file):
    """project.rs linked_repo: follow a .git file to the main repository."""
    text = _small(git_file)
    gitdir = next((ln.strip()[len("gitdir:"):].strip() for ln in (text or "").splitlines()
                   if ln.strip().startswith("gitdir:")), None)
    if gitdir is None:
        return None
    gitdir = Path(worktree) / gitdir
    common = (_small(gitdir / "commondir") or "").strip()
    if common:
        return _common_repo(gitdir / common.splitlines()[0])
    if gitdir.parent.name.lower() != "worktrees":
        return None  # a submodule: its own name
    return _common_repo(gitdir.parent.parent)


def _git_repo(cwd, home=None):
    """The repository of the nearest .git at or above ``cwd``, below
    ``home`` (project.rs repo_name; a UNC path is not walked)."""
    if len(cwd) >= 2 and cwd[0] in "\\/" and cwd[1] in "\\/":
        return None
    d = Path(cwd)
    for p in [d, *d.parents][:40]:
        if _same_folder(p, home):
            break
        git = p / ".git"
        if git.is_dir():
            if (git / "HEAD").is_file():
                return p.name or None
        elif git.is_file():
            return _linked_repo(p, git) or p.name or None
    return None


def _project(cwd, home=None):
    cwd = cwd or ""
    if not cwd.strip():
        return ""
    parts = cwd.replace("/", "\\").split("\\")
    # The first .claude/worktrees: a worktree made inside another names the
    # outer repository.
    for i in range(len(parts) - 2):
        if (parts[i].lower(), parts[i + 1].lower()) == (".claude", "worktrees") and parts[i + 2]:
            repo = parts[i - 1] if i else ""
            if repo and not repo.endswith(":"):
                return repo
            break
    return _git_repo(cwd, home) or ntpath.basename(cwd.rstrip("/\\"))


def translate(msgs, payload=None, env=None):
    """Python hook messages -> protocol v1 messages (module docs)."""
    home = (env or {}).get("USERPROFILE")
    out = _translate(msgs, home)
    label = _project((payload or {}).get("cwd"), home)
    if label:
        for d in out:
            if d["type"] in LABELLED:
                d["label"] = label
    return out


def _translate(msgs, home=None):
    out = []
    for m in msgs:
        t = m["type"]
        if t == MsgType.PROSE:
            out.append(dict(_base("stream", m), delta=m["delta"], index=m["index"],
                            final=m["final"]))
        elif t == MsgType.EARCON:
            if m["kind"] == "turn_done":
                out.append(_base("turn_end", m))
        elif t == MsgType.CHOICE:
            out.extend(_questions(m))
        elif t == MsgType.PLAN:
            out.append(_hinted(dict(_base("ask", m), kind="plan", text=m["text"])))
        elif t == MsgType.PERMISSION:
            text = (m.get("action") or "").strip() or (m.get("message") or "").strip()
            out.append(_hinted(dict(_base("ask", m), kind="permission", text=text)))
        elif t == MsgType.TOOL:
            out.append(dict(_base("tool", m), name=m["tool"] or "", summary=m["summary"]))
        elif t == MsgType.CHOICE_ANSWERED:
            out.append(_base("answered", m))
        elif t == MsgType.SET_FOREGROUND:
            op = _base("channel_open", m)
            project = _project(m.get("cwd"), home)
            if project:
                op["label"] = project
                op["keep_label"] = True
            if m.get("host_tab"):
                op["host_tab"] = m["host_tab"]
            out.extend([op, _base("focus", m)])
        elif t == MsgType.FLUSH:
            out.append(_base("turn_start", m))
        elif t == MsgType.SESSION_START:
            pass
        elif t == MsgType.SESSION_END:
            out.append(_base("channel_close", m))
        else:
            raise AssertionError(f"no adaptation for {t}")
    return out


def _cases():
    return sorted(GOLDEN.glob("*.json"))


def test_there_are_golden_cases_for_every_captured_payload():
    used = {json.loads(p.read_text(encoding="utf-8")).get("fixture") for p in _cases()}
    assert {p.name for p in FIXTURES.glob("*.json")} <= used


def test_the_hints_are_the_python_daemons_selection_cue():
    src = INGEST.read_text(encoding="utf-8")
    assert HINT in src and ONCE in src


@pytest.mark.parametrize("path", _cases(), ids=lambda p: p.stem)
def test_golden_messages_are_the_python_mapping(path):
    case = json.loads(path.read_text(encoding="utf-8"))
    payload = _payload(case)
    msgs = handle_event(case["event"], payload, env=case.get("env", {}))
    got = translate(msgs, payload, case.get("env", {}))
    if os.environ.get("SONARA_REGEN_GOLDEN"):
        case["messages"] = got
        path.write_text(json.dumps(case, indent=2) + "\n", encoding="utf-8")
    assert got == case["messages"]


def test_the_project_mirror_follows_the_rust_rules(tmp_path):
    """_project is project.rs: nested Claude worktrees, a linked worktree's
    main repository, a bare common folder, a submodule (#245)."""
    nested = r"C:\nowhere-245\Filesmith\.claude\worktrees\a\.claude\worktrees\b"
    assert _project(nested) == "Filesmith"
    assert _project(r"\server-245\share\proj\src") == "src"
    main = tmp_path / "PrismTerminal"
    admin = main / ".git" / "worktrees" / "agent-hooks"
    admin.mkdir(parents=True)
    (admin / "commondir").write_text("../..\n")
    wt = tmp_path / "elsewhere" / "agent-hooks"
    (wt / "src").mkdir(parents=True)
    (wt / ".git").write_text(f"gitdir: {admin}\n")
    assert _project(str(wt / "src")) == "PrismTerminal"
    (admin / "commondir").unlink()
    assert _project(str(wt)) == "PrismTerminal"
    bare = tmp_path / "tool.git" / "worktrees" / "w"
    bare.mkdir(parents=True)
    (bare / "commondir").write_text("../..")
    w = tmp_path / "w"
    w.mkdir()
    (w / ".git").write_text(f"gitdir: {bare}")
    assert _project(str(w)) == "tool"
    sub = tmp_path / "super" / "lib"
    (tmp_path / "super" / ".git" / "modules" / "lib").mkdir(parents=True)
    sub.mkdir()
    (sub / ".git").write_text("gitdir: ../.git/modules/lib")
    assert _project(str(sub)) == "lib"
