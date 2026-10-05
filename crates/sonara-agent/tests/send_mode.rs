//! Send mode `message` (#235): with an engine that takes whole messages
//! (`Rules::set_whole_messages`, set by the driver from the reader), every
//! release of prose is ONE `Speak` (one channel entry, one reader item, one
//! request): the turn end in read mode `done`, a batch in `queue`, a
//! finished paragraph in `immediate`, the prose held before a decision.
//! Paragraph breaks are kept as a blank line. Decisions stay their own
//! short `Speak`. Without it (`sentence`) every chunk is its own `Speak`,
//! as before.
use sonara_agent::settings::Settings;
use sonara_agent::{Action, Ask, AskKind, Choice, ReadMode, Rules};

fn rules(mode: ReadMode, whole: bool) -> Rules {
    let mut r = Rules::new(Settings {
        read_mode: mode,
        minqueue: 3,
        ..Default::default()
    });
    r.set_whole_messages(whole);
    r
}

/// The texts spoken, `!` marking a decision.
fn spoken(actions: &[Action]) -> Vec<String> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Speak { text, decision, .. } => {
                Some(format!("{}{text}", if *decision { "!" } else { "" }))
            }
            _ => None,
        })
        .collect()
}

fn prose(r: &mut Rules, delta: &str, index: u32, fin: bool) -> Vec<Action> {
    r.stream("fg", None, delta, index, fin, None).unwrap()
}

/// 18 sentences in three paragraphs, as the reply of the evidence
/// (2026-10-04: 18 entries, 18 Gemini requests).
fn reply() -> (String, Vec<String>) {
    let words = [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
    ];
    let s: Vec<String> = words.iter().map(|w| format!("This is line {w}.")).collect();
    let paras: Vec<String> = s.chunks(6).map(|c| c.join(" ")).collect();
    (paras.join("\n\n"), s)
}

#[test]
fn done_mode_releases_the_whole_reply_as_one_speak() {
    let (text, sentences) = reply();
    let mut whole = rules(ReadMode::Done, true);
    let mut each = rules(ReadMode::Done, false);
    for r in [&mut whole, &mut each] {
        // Streamed a sentence at a time.
        for word in text.split_inclusive('.') {
            assert!(spoken(&prose(r, word, 0, false)).is_empty());
        }
        assert!(spoken(&prose(r, "", 0, true)).is_empty());
    }
    let one = spoken(&whole.turn_end("fg", None, None).unwrap());
    assert_eq!(one, vec![text.clone()], "one Speak, paragraphs kept");
    let many = spoken(&each.turn_end("fg", None, None).unwrap());
    assert_eq!(many, sentences, "sentence mode: one Speak per sentence");
}

#[test]
fn separate_blocks_are_separate_paragraphs() {
    let mut r = rules(ReadMode::Done, true);
    prose(&mut r, "First block. Still first.", 0, true);
    prose(&mut r, "Second block.", 1, true);
    assert_eq!(
        spoken(&r.turn_end("fg", None, None).unwrap()),
        ["First block. Still first.\n\nSecond block."]
    );
}

/// `immediate` with whole messages: a paragraph is the release point (its
/// blank line, or the end of its block), so speech starts after the first
/// paragraph and each paragraph is one request.
#[test]
fn immediate_mode_releases_each_finished_paragraph() {
    let mut r = rules(ReadMode::Immediate, true);
    assert!(spoken(&prose(&mut r, "One. Two. ", 0, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "\n\nThree. ", 0, false)),
        ["One. Two."],
        "the blank line ends the paragraph; the next one waits"
    );
    assert!(spoken(&prose(&mut r, "Four. ", 0, false)).is_empty());
    // The end of the block ends the paragraph too.
    assert_eq!(
        spoken(&prose(&mut r, "Five.", 0, true)),
        ["Three. Four. Five."]
    );
    // Several finished paragraphs in one delta go out as they are ready,
    // together: one release point.
    assert_eq!(
        spoken(&prose(&mut r, "Six.\n\nSeven.\n\nEight", 1, false)),
        ["Six.\n\nSeven."]
    );
    // "Eight" is no sentence yet; its block's end reads it.
    assert!(spoken(&r.turn_end("fg", None, None).unwrap()).is_empty());
    assert_eq!(spoken(&prose(&mut r, "", 1, true)), ["Eight"]);
}

#[test]
fn immediate_sentence_mode_is_unchanged() {
    let mut r = rules(ReadMode::Immediate, false);
    assert_eq!(
        spoken(&prose(&mut r, "One. Two. ", 0, false)),
        ["One.", "Two."]
    );
}

#[test]
fn queue_mode_releases_a_batch_as_one_speak() {
    let mut r = rules(ReadMode::Queue, true);
    assert!(spoken(&prose(&mut r, "One. Two. ", 0, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "Three. ", 0, false)),
        ["One. Two. Three."]
    );
    // A tool run releases what is held, then is announced on its own.
    r.settings.verbosity = sonara_agent::Verbosity::Everything;
    prose(&mut r, "Looking. ", 1, false);
    assert_eq!(spoken(&r.tool("fg", "Bash", "ls")), ["Looking.", "ls"]);
}

/// The held prose before a decision is one request; the decision is its
/// own short one (it is read with priority, so it stays its own entry).
#[test]
fn the_prose_before_a_decision_is_one_speak_and_the_decision_its_own() {
    let mut r = rules(ReadMode::Done, true);
    prose(&mut r, "Two ways to go. Both work.\n\nPick one. ", 0, false);
    let mut q = Ask::new(AskKind::Question, "Which one?");
    q.options = vec![
        Choice {
            label: "A".into(),
            description: None,
        },
        Choice {
            label: "B".into(),
            description: None,
        },
    ];
    let a = r.ask("fg", &q);
    assert_eq!(
        spoken(&a),
        [
            "Two ways to go. Both work.\n\nPick one.",
            "!Which one? Option 1: A. Option 2: B."
        ]
    );
}

#[test]
fn a_new_turn_or_a_flush_drops_the_held_message() {
    let mut r = rules(ReadMode::Done, true);
    prose(&mut r, "Old reply. ", 0, false);
    r.turn_start("fg", None, None).unwrap();
    assert!(spoken(&r.turn_end("fg", None, None).unwrap()).is_empty());
    r.turn_start("fg", None, None).unwrap();
    prose(&mut r, "Flushed reply. ", 0, false);
    assert!(spoken(&r.flush("fg")).is_empty());
    assert!(spoken(&r.turn_end("fg", None, None).unwrap()).is_empty());
}

#[test]
fn muted_the_joined_message_is_stored_not_spoken() {
    // #243: kept whole as the session's latest message, not read.
    let mut r = rules(ReadMode::Done, true);
    r.set_mute_level(1);
    prose(&mut r, "One. Two.", 0, true);
    let a = r.turn_end("fg", None, None).unwrap();
    assert!(spoken(&a).is_empty());
    let stored: Vec<&Action> = a
        .iter()
        .filter(|a| matches!(a, Action::Store { .. }))
        .collect();
    assert_eq!(
        stored,
        [&Action::Store {
            channel: "fg".into(),
            text: "One. Two.".into(),
            decision: false,
            kind: "prose",
        }]
    );
}

/// A tool run releases the prose held so far, but the release rules keep
/// holding after it (review of #236): in `immediate` the next paragraph is
/// still one request, not one per delta. Deltas are numbered 0, 1, 2, ...
/// within a message block, as Claude Code's MessageDisplay sends them.
#[test]
fn immediate_whole_keeps_paragraphs_after_a_tool_run() {
    let mut r = rules(ReadMode::Immediate, true);
    r.settings.verbosity = sonara_agent::Verbosity::Everything;
    prose(&mut r, "Let me look. ", 0, true);
    assert_eq!(spoken(&r.tool("fg", "Bash", "ls")), ["ls"]);
    assert!(spoken(&prose(&mut r, "Found it. ", 0, false)).is_empty());
    assert!(spoken(&prose(&mut r, "It is here. ", 1, false)).is_empty());
    assert!(spoken(&prose(&mut r, "All good. ", 2, false)).is_empty());
    assert_eq!(
        spoken(&prose(
            &mut r, "

Next. ", 3, false
        )),
        ["Found it. It is here. All good."],
        "one paragraph, one Speak, no blank lines between deltas"
    );
}

/// `queue` in whole messages batches at minqueue again after a tool run.
#[test]
fn queue_whole_batches_again_after_a_tool_run() {
    let mut r = rules(ReadMode::Queue, true);
    r.settings.verbosity = sonara_agent::Verbosity::Everything;
    prose(&mut r, "Looking. ", 0, false);
    assert_eq!(spoken(&r.tool("fg", "Bash", "ls")), ["Looking.", "ls"]);
    assert!(spoken(&prose(&mut r, "One. ", 0, false)).is_empty());
    assert!(spoken(&prose(&mut r, "Two. ", 1, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "Three. ", 2, false)),
        ["One. Two. Three."]
    );
    // The turn end still releases the rest.
    prose(&mut r, "Four. ", 3, false);
    assert_eq!(spoken(&r.turn_end("fg", None, None).unwrap()), ["Four."]);
}

/// A new delta of the same block is no new paragraph; a new block (its
/// deltas restart at 0) is.
#[test]
fn deltas_of_one_block_join_with_a_space_a_new_block_with_a_blank_line() {
    let mut r = rules(ReadMode::Done, true);
    prose(&mut r, "One. ", 0, false);
    prose(&mut r, "Two. ", 1, false);
    prose(&mut r, "Three. ", 2, false);
    prose(&mut r, "Four. ", 0, false);
    prose(&mut r, "Five.", 1, true);
    assert_eq!(
        spoken(&r.turn_end("fg", None, None).unwrap()),
        ["One. Two. Three.

Four. Five."]
    );
}
