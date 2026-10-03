//! Spoken text of a decision (`ask`): a question with its options, a plan
//! ready for review, a permission prompt. Pure functions, ported from the
//! Python daemon's `decision_text.py`; host-specific key instructions (how
//! to pick an option in the Claude Code TUI) are not here, an adapter sends
//! them as `notes` and `hint`.

/// One option of a question.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub description: Option<String>,
}

/// What kind of decision blocks the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    Question,
    Permission,
    Plan,
}

impl AskKind {
    /// The protocol names `question`, `permission` and `plan`.
    pub fn parse(name: &str) -> Option<AskKind> {
        match name {
            "question" => Some(AskKind::Question),
            "permission" => Some(AskKind::Permission),
            "plan" => Some(AskKind::Plan),
            _ => None,
        }
    }

    /// The protocol name.
    pub fn as_str(&self) -> &'static str {
        match self {
            AskKind::Question => "question",
            AskKind::Permission => "permission",
            AskKind::Plan => "plan",
        }
    }
}

fn ends_sentence(s: &str) -> bool {
    s.ends_with(['.', '!', '?'])
}

/// A question: its text, then "Option n: label." for each option (with its
/// description), numbered as the host shows them (an option without a
/// label is skipped but keeps its number). `multi` says that several may
/// be picked.
pub fn question_text(text: &str, options: &[Choice], multi: bool) -> String {
    let text = text.trim();
    let mut segs = Vec::new();
    for (i, o) in options.iter().enumerate() {
        if o.label.is_empty() {
            continue;
        }
        let mut seg = format!("Option {}: {}.", i + 1, o.label);
        let desc = o.description.as_deref().map(str::trim).unwrap_or("");
        if !desc.is_empty() {
            seg.push(' ');
            seg.push_str(desc);
            if !ends_sentence(desc) {
                seg.push('.');
            }
        }
        segs.push(seg);
    }
    let head = if multi {
        let mut h = String::new();
        if !text.is_empty() {
            h.push_str(text);
            h.push(' ');
        }
        h.push_str("This is a multi-select; you can pick more than one.");
        h
    } else {
        text.to_string()
    };
    match (head.is_empty(), segs.is_empty()) {
        (false, false) => format!("{head} {}", segs.join(" ")),
        (true, false) => segs.join(" "),
        (false, true) => head,
        (true, true) => "A question needs your answer.".into(),
    }
}

/// A plan ready for review.
pub fn plan_text(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        "A plan is ready for your review.".into()
    } else {
        format!("Plan ready. {text}")
    }
}

/// A permission prompt: the pending action (the permission earcon already
/// says approval is needed).
pub fn permission_text(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        "Permission needed.".into()
    } else {
        text.to_string()
    }
}

/// `base` followed by the non-empty `extras`, one space apart.
pub fn with_extras(base: String, extras: &[&str]) -> String {
    let extras: Vec<&str> = extras
        .iter()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .collect();
    if extras.is_empty() {
        base
    } else {
        format!("{base} {}", extras.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(label: &str, desc: Option<&str>) -> Choice {
        Choice {
            label: label.into(),
            description: desc.map(str::to_string),
        }
    }

    #[test]
    fn a_question_lists_its_options_with_descriptions() {
        assert_eq!(
            question_text(
                "Which color do you prefer?",
                &[opt("Red", Some("warm")), opt("Blue", Some("cool!"))],
                false
            ),
            "Which color do you prefer? Option 1: Red. warm. Option 2: Blue. cool!"
        );
    }

    #[test]
    fn an_empty_label_keeps_the_numbering() {
        assert_eq!(
            question_text("Pick", &[opt("", None), opt("B", None)], false),
            "Pick Option 2: B."
        );
    }

    #[test]
    fn multi_select_and_empty_questions() {
        assert_eq!(
            question_text("Pick", &[opt("A", None)], true),
            "Pick This is a multi-select; you can pick more than one. Option 1: A."
        );
        assert_eq!(
            question_text("", &[], true),
            "This is a multi-select; you can pick more than one."
        );
        assert_eq!(
            question_text(" ", &[], false),
            "A question needs your answer."
        );
        assert_eq!(question_text("", &[opt("A", None)], false), "Option 1: A.");
    }

    #[test]
    fn plans_and_permissions() {
        assert_eq!(plan_text(" Step one. "), "Plan ready. Step one.");
        assert_eq!(plan_text(""), "A plan is ready for your review.");
        assert_eq!(permission_text("Run git status"), "Run git status");
        assert_eq!(permission_text("  "), "Permission needed.");
    }

    #[test]
    fn extras_join_with_one_space() {
        assert_eq!(with_extras("Q?".into(), &["", " a. ", "b."]), "Q? a. b.");
        assert_eq!(with_extras("Q?".into(), &[]), "Q?");
    }
}
