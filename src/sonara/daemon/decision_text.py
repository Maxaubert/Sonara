"""Spoken text for decisions: the AskUserQuestion choice, its keyboard notes,
a plan ready for review and a permission prompt. Pure functions of the hook
message, so they hold no daemon state."""
from __future__ import annotations


def choice_text(msg) -> str:
    parts = []
    for q in msg.get("questions", []) or []:
        qtext = q.get("question", "") if isinstance(q, dict) else str(q)
        multi = bool(isinstance(q, dict) and q.get("multiSelect"))
        opts = q.get("options", []) if isinstance(q, dict) else []
        segs = []
        for i, o in enumerate(opts, 1):
            if isinstance(o, dict):
                label = o.get("label", "")
                desc = (o.get("description") or "").strip()
            else:
                label, desc = str(o), ""
            if not label:
                continue   # keep numbering aligned with the TUI's digits
            seg = "Option {0}: {1}.".format(i, label)
            if desc:
                seg += " {0}{1}".format(
                    desc, "" if desc.endswith((".", "!", "?")) else ".")
            segs.append(seg)
        head = qtext
        if multi:
            head = "{0}{1}".format(
                (qtext + " ") if qtext else "",
                "This is a multi-select; you can pick more than one.")
        if head and segs:
            parts.append("{0} {1}".format(head, " ".join(segs)))
        elif segs:
            parts.append(" ".join(segs))
        elif head:
            parts.append(head)
    return " ".join(parts) if parts else "A question needs your answer."


def choice_notes(msg) -> str:
    notes = []
    questions = msg.get("questions", []) or []
    if any(isinstance(q, dict) and q.get("multiSelect") for q in questions):
        notes.append(
            "Select multiple: press each number, or Space on the "
            "highlighted item, then Enter to confirm."
        )
    if any(
        isinstance(q, dict) and len(q.get("options", []) or []) > 9
        for q in questions
    ):
        notes.append("More than nine options; use arrow keys for ten and up.")
    return " ".join(notes)


def plan_text(msg) -> str:
    text = (msg.get("text") or "").strip()
    if text:
        return "Plan ready. {0}".format(text)
    return "A plan is ready for your review."


def permission_text(msg) -> str:
    # The 'permission' earcon already signals approval is needed; speak the
    # pending action, else the human-readable message, else a generic cue.
    action = (msg.get("action") or "").strip()
    if action:
        return action
    message = (msg.get("message") or "").strip()
    return message if message else "Permission needed."
