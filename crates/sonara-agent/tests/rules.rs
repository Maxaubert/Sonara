//! The pure L3 rules, ported from the Python daemon's behaviour tests
//! (`test_daemon_prose.py`, `test_daemon_flush_late_prose.py`,
//! `test_daemon_minqueue.py`, `test_daemon_decisions.py`,
//! `test_daemon_decision_dedup.py`, `test_daemon_pause_mute.py`,
//! `test_daemon_question_flow.py`, `test_daemon_summary_mode.py`,
//! `test_summary_pipeline.py`, `test_digest_reorder.py`). Issue numbers name
//! the Python regressions.
use sonara_agent::settings::Settings;
use sonara_agent::{
    Action, Ask, AskKind, Choice, Earcon, Job, ReadMode, Rules, Stale, Step, Timer, Verbosity,
};
use std::time::Duration;

fn rules() -> Rules {
    Rules::new(Settings::default())
}

fn summary_rules() -> Rules {
    let mut s = Settings::default();
    s.summaries.enabled = true;
    Rules::new(s)
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

fn earcons(actions: &[Action]) -> Vec<Earcon> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Earcon(e) => Some(*e),
            _ => None,
        })
        .collect()
}

fn jobs(actions: &[Action]) -> Vec<Job> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Summarize(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

fn timers(actions: &[Action]) -> Vec<(Duration, Timer)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Timer { after, timer } => Some((*after, timer.clone())),
            _ => None,
        })
        .collect()
}

fn settle_of(actions: &[Action]) -> Timer {
    timers(actions)
        .into_iter()
        .map(|(_, t)| t)
        .find(|t| matches!(t, Timer::Settle { .. }))
        .expect("a settle timer")
}

fn prose(r: &mut Rules, ch: &str, delta: &str, index: u32, fin: bool) -> Vec<Action> {
    r.stream(ch, None, delta, index, fin, None).unwrap()
}

fn prose_t(r: &mut Rules, ch: &str, delta: &str, index: u32, fin: bool, t: f64) -> Vec<Action> {
    r.stream(ch, None, delta, index, fin, Some(t))
        .unwrap_or_default()
}

fn question(text: &str, options: &[&str]) -> Ask {
    let mut a = Ask::new(AskKind::Question, text);
    a.options = options
        .iter()
        .map(|l| Choice {
            label: l.to_string(),
            description: None,
        })
        .collect();
    a
}

const PAD: &str = "This filler sentence carries the turn well past the threshold. ";

// -- prose and turns ------------------------------------------------------

#[test]
fn prose_is_spoken_one_chunk_per_sentence() {
    let mut r = rules();
    let a = prose(&mut r, "fg", "Hello there. How are you? ", 0, false);
    assert_eq!(spoken(&a), ["Hello there.", "How are you?"]);
    assert!(matches!(&a[0], Action::Speak { channel, .. } if channel == "fg"));
}

#[test]
fn a_partial_sentence_waits_for_the_final_flag() {
    let mut r = rules();
    assert!(spoken(&prose(&mut r, "fg", "Half a sen", 0, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "fg", "tence", 1, true)),
        ["Half a sentence"]
    );
}

#[test]
fn each_channel_has_its_own_assembler() {
    let mut r = rules();
    prose(&mut r, "a", "Alpha ", 0, false);
    prose(&mut r, "b", "Beta ", 0, false);
    assert_eq!(spoken(&prose(&mut r, "a", "one.", 1, true)), ["Alpha one."]);
    assert_eq!(spoken(&prose(&mut r, "b", "two.", 1, true)), ["Beta two."]);
}

#[test]
fn prose_is_spoken_at_every_verbosity() {
    for v in [Verbosity::Everything, Verbosity::SkipCode] {
        let mut r = rules();
        r.settings.verbosity = v;
        let a = prose(&mut r, "fg", "Hello. ", 0, true);
        assert_eq!(spoken(&a), ["Hello."], "{v:?}");
    }
}

#[test]
fn everything_announces_a_code_block_and_skip_code_drops_it_silently() {
    let text = "Here it is.\n```python\nprint(1)\nprint(2)\nprint(3)\n```\nDone.\n";
    let mut r = rules();
    r.settings.verbosity = Verbosity::Everything;
    let a = prose(&mut r, "fg", text, 0, true);
    assert_eq!(
        spoken(&a),
        ["Here it is.", "3-line python code block", "Done."]
    );
    let mut r = rules();
    r.settings.verbosity = Verbosity::SkipCode;
    let a = prose(&mut r, "fg", text, 0, true);
    assert_eq!(spoken(&a), ["Here it is.", "Done."]);
}

#[test]
fn verbosity_names_and_their_old_aliases() {
    assert_eq!(Verbosity::parse("everything"), Some(Verbosity::Everything));
    assert_eq!(Verbosity::parse("all"), Some(Verbosity::Everything));
    for old in ["skip_code", "medium", "quiet"] {
        assert_eq!(Verbosity::parse(old), Some(Verbosity::SkipCode), "{old}");
    }
    assert_eq!(Verbosity::parse("loud"), None);
    assert_eq!(Verbosity::Everything.as_str(), "everything");
    assert_eq!(Verbosity::SkipCode.as_str(), "skip_code");
}

#[test]
fn a_new_turn_wipes_the_channel_and_resets_the_assembler() {
    let mut r = rules();
    prose(&mut r, "fg", "Unfinished old sen", 0, false);
    let a = r.turn_start("fg", None, None).unwrap();
    assert_eq!(
        a,
        [Action::Wipe {
            channel: "fg".into(),
            resume: true
        }]
    );
    assert_eq!(
        spoken(&prose(&mut r, "fg", "New text.", 0, true)),
        ["New text."]
    );
}

#[test]
fn late_old_prose_after_a_new_turn_is_not_read() {
    // #174 test_late_old_prose_after_flush_is_not_read
    let mut r = rules();
    prose_t(&mut r, "fg", "Old one. Old two. ", 0, false, 100.0);
    r.turn_start("fg", None, Some(105.0)).unwrap();
    assert_eq!(
        r.stream("fg", None, "Old three. Old four.", 1, true, Some(104.0)),
        Err(Stale)
    );
    assert_eq!(r.turn_end("fg", None, Some(104.5)), Err(Stale));
}

#[test]
fn a_late_old_block_at_index_zero_does_not_lead_the_new_turn() {
    let mut r = rules();
    prose_t(&mut r, "fg", "Old one. ", 0, false, 100.0);
    r.turn_end("fg", None, Some(101.0)).unwrap();
    r.turn_start("fg", None, Some(105.0)).unwrap();
    assert!(r
        .stream("fg", None, "Late tail. Late two.", 0, true, Some(103.0))
        .is_err());
    let a = prose_t(&mut r, "fg", "New A. New B. ", 0, false, 106.0);
    assert_eq!(spoken(&a), ["New A.", "New B."]);
}

#[test]
fn a_stale_index_does_not_swallow_the_new_turns_text() {
    let mut r = rules();
    r.turn_start("fg", None, Some(105.0)).unwrap();
    assert!(r
        .stream("fg", None, "Stale tail. ", 2, false, Some(104.0))
        .is_err());
    let mut heard = Vec::new();
    for (i, text) in ["New A. ", "New B. ", "New C. ", "New D. "]
        .iter()
        .enumerate()
    {
        let a = prose_t(&mut r, "fg", text, i as u32, i == 3, 106.0 + i as f64);
        heard.extend(spoken(&a));
    }
    assert_eq!(heard, ["New A.", "New B.", "New C.", "New D."]);
}

#[test]
fn a_late_old_turn_end_plays_no_chime_and_does_not_release_the_new_turn() {
    let mut r = rules();
    r.settings.minqueue = 5;
    r.turn_start("fg", None, Some(105.0)).unwrap();
    assert!(spoken(&prose_t(&mut r, "fg", "New A. ", 0, false, 106.0)).is_empty());
    assert_eq!(r.turn_end("fg", None, Some(104.0)), Err(Stale));
    // The held prose is still held: only the real turn end releases it.
    let a = r.turn_end("fg", None, Some(107.0)).unwrap();
    assert_eq!(spoken(&a), ["New A."]);
    assert_eq!(earcons(&a), [Earcon::TurnDone]);
}

#[test]
fn unstamped_messages_are_never_stale() {
    let mut r = rules();
    r.turn_start("fg", None, Some(105.0)).unwrap();
    assert_eq!(
        spoken(&prose(&mut r, "fg", "Hello there.", 0, true)),
        ["Hello there."]
    );
}

#[test]
fn a_new_turn_only_guards_its_own_channel() {
    let mut r = rules();
    r.turn_start("fg", None, Some(105.0)).unwrap();
    let a = prose_t(&mut r, "other", "Other reply.", 0, true, 104.0);
    assert_eq!(spoken(&a), ["Other reply."]);
}

#[test]
fn text_naming_an_earlier_turn_is_dropped() {
    let mut r = rules();
    r.turn_start("fg", Some("t1"), None).unwrap();
    assert!(r.stream("fg", Some("t1"), "One. ", 0, false, None).is_ok());
    r.turn_start("fg", Some("t2"), None).unwrap();
    assert_eq!(
        r.stream("fg", Some("t1"), "Late. ", 0, true, None),
        Err(Stale)
    );
    assert_eq!(r.turn_end("fg", Some("t1"), None), Err(Stale));
    assert_eq!(r.turn_start("fg", Some("t1"), None), Err(Stale));
    let a = r
        .stream("fg", Some("t2"), "Fresh. ", 0, true, None)
        .unwrap();
    assert_eq!(spoken(&a), ["Fresh."]);
}

#[test]
fn an_older_turn_start_is_dropped() {
    let mut r = rules();
    r.turn_start("fg", None, Some(110.0)).unwrap();
    assert_eq!(r.turn_start("fg", None, Some(105.0)), Err(Stale));
    assert!(r.turn_start("fg", None, Some(110.0)).is_ok());
}

// -- minqueue --------------------------------------------------------------

#[test]
fn prose_is_held_below_minqueue_then_flushed_at_once() {
    let mut r = rules();
    r.settings.minqueue = 3;
    assert!(spoken(&prose(&mut r, "fg", "One. Two. ", 0, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "fg", "Three. ", 1, false)),
        ["One.", "Two.", "Three."]
    );
}

#[test]
fn a_blocks_final_is_not_the_turn_boundary() {
    let mut r = rules();
    r.settings.minqueue = 5;
    assert!(spoken(&prose(&mut r, "fg", "Alpha one. Alpha two. ", 0, true)).is_empty());
    assert!(spoken(&prose(&mut r, "fg", "Beta one. Beta two. ", 0, true)).is_empty());
    let a = r.turn_end("fg", None, None).unwrap();
    assert_eq!(
        spoken(&a),
        ["Alpha one.", "Alpha two.", "Beta one.", "Beta two."]
    );
    // After the turn end the rest of the turn flows at once.
    assert_eq!(spoken(&prose(&mut r, "fg", "Late. ", 1, false)), ["Late."]);
}

#[test]
fn minqueue_zero_and_one_read_at_once() {
    for n in [0, 1] {
        let mut r = rules();
        r.settings.minqueue = n;
        assert_eq!(
            spoken(&prose(&mut r, "fg", "Hi there. ", 0, false)),
            ["Hi there."]
        );
    }
}

#[test]
fn a_tool_announcement_follows_the_held_prose() {
    let mut r = rules();
    r.settings.minqueue = 5;
    prose(&mut r, "fg", "Looking now. ", 0, false);
    let a = r.tool("fg", "Bash", "ls");
    assert_eq!(spoken(&a), ["Looking now.", "ls"]);
    assert_eq!(spoken(&r.tool("fg", "Bash", " ")), ["Running Bash."]);
}

#[test]
fn tools_are_announced_at_everything_and_suppressed_at_skip_code() {
    for (v, n) in [(Verbosity::Everything, 1), (Verbosity::SkipCode, 0)] {
        let mut r = rules();
        r.settings.verbosity = v;
        assert_eq!(spoken(&r.tool("fg", "Bash", "ls")).len(), n, "{v:?}");
    }
}

#[test]
fn a_new_turn_discards_held_prose() {
    let mut r = rules();
    r.settings.minqueue = 3;
    prose(&mut r, "fg", "One. Two. ", 0, false);
    r.turn_start("fg", None, None).unwrap();
    let a = r.turn_end("fg", None, None).unwrap();
    assert!(spoken(&a).is_empty());
}

// -- read_mode (#222) -------------------------------------------------------

fn mode_rules(mode: ReadMode) -> Rules {
    let mut r = rules();
    r.settings.read_mode = mode;
    r.settings.minqueue = 5;
    r
}

#[test]
fn read_mode_immediate_speaks_each_chunk_whatever_the_queue_size() {
    let mut r = mode_rules(ReadMode::Immediate);
    assert_eq!(spoken(&prose(&mut r, "fg", "One. ", 0, false)), ["One."]);
    assert_eq!(spoken(&prose(&mut r, "fg", "Two. ", 0, false)), ["Two."]);
}

#[test]
fn read_mode_queue_holds_until_the_queue_size_waits() {
    let mut r = mode_rules(ReadMode::Queue);
    r.settings.minqueue = 3;
    assert!(spoken(&prose(&mut r, "fg", "One. Two. ", 0, false)).is_empty());
    assert_eq!(
        spoken(&prose(&mut r, "fg", "Three. ", 1, false)),
        ["One.", "Two.", "Three."]
    );
}

#[test]
fn read_mode_queue_is_released_by_a_tool_run() {
    let mut r = mode_rules(ReadMode::Queue);
    prose(&mut r, "fg", "Looking now. ", 0, false);
    assert_eq!(spoken(&r.tool("fg", "Bash", "ls")), ["Looking now.", "ls"]);
    assert_eq!(
        spoken(&prose(&mut r, "fg", "Found it. ", 1, false)),
        ["Found it."]
    );
}

#[test]
fn read_mode_done_holds_all_prose_until_the_turn_end() {
    let mut r = mode_rules(ReadMode::Done);
    for i in 0..12 {
        let a = prose(&mut r, "fg", &format!("Sentence {i}. "), i, true);
        assert!(spoken(&a).is_empty(), "chunk {i} held");
    }
    let a = r.turn_end("fg", None, None).unwrap();
    assert_eq!(spoken(&a).len(), 12);
    assert_eq!(spoken(&a)[0], "Sentence 0.");
    assert_eq!(earcons(&a), [Earcon::TurnDone]);
    // After the turn end late prose flows at once.
    assert_eq!(spoken(&prose(&mut r, "fg", "Late. ", 20, false)), ["Late."]);
}

#[test]
fn read_mode_done_keeps_holding_through_a_tool_run() {
    let mut r = mode_rules(ReadMode::Done);
    prose(&mut r, "fg", "Looking now. ", 0, false);
    // The tool is announced, the prose stays held.
    assert_eq!(spoken(&r.tool("fg", "Bash", "ls")), ["ls"]);
    assert!(spoken(&prose(&mut r, "fg", "Found it. ", 1, false)).is_empty());
    let a = r.turn_end("fg", None, None).unwrap();
    assert_eq!(spoken(&a), ["Looking now.", "Found it."]);
}

#[test]
fn read_mode_done_flushes_held_prose_before_a_question() {
    let mut r = mode_rules(ReadMode::Done);
    prose(&mut r, "fg", "Two ways to go. ", 0, false);
    let a = r.ask("fg", &question("Which one?", &["A", "B"]));
    assert_eq!(
        spoken(&a),
        ["Two ways to go.", "!Which one? Option 1: A. Option 2: B."]
    );
    assert_eq!(earcons(&a), [Earcon::Choice]);
}

#[test]
fn read_mode_done_flushes_held_prose_before_a_permission_and_a_plan() {
    for (kind, said) in [
        (AskKind::Permission, "!Run ls"),
        (AskKind::Plan, "!Plan ready. Run ls"),
    ] {
        let mut r = mode_rules(ReadMode::Done);
        prose(&mut r, "fg", "Context first. ", 0, false);
        let a = r.ask("fg", &Ask::new(kind, "Run ls"));
        assert_eq!(spoken(&a), ["Context first.", said], "{kind:?}");
        // The turn goes on holding after the decision.
        assert!(spoken(&prose(&mut r, "fg", "More. ", 1, false)).is_empty());
    }
}

#[test]
fn read_mode_done_held_prose_is_noted() {
    let mut r = mode_rules(ReadMode::Done);
    assert!(prose(&mut r, "fg", "One.", 0, true).is_empty());
    let n = notes(&r);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(
        n[0].1
            .contains("held: waits for the turn end (read_mode done)"),
        "{n:?}"
    );
}

#[test]
fn read_mode_done_held_prose_dropped_by_an_answer_or_a_new_turn_is_noted() {
    // Review of #222: done holds a whole turn, so a drop must show in the log.
    let mut r = mode_rules(ReadMode::Done);
    prose(&mut r, "fg", "One. Two. ", 0, false);
    let _ = notes(&r);
    r.answered("fg");
    let n = notes(&r);
    assert!(
        n.iter()
            .any(|(_, w, _)| w == "dropped: 2 held chunk(s) (answered, read_mode done)"),
        "{n:?}"
    );
    prose(&mut r, "fg", "Three. ", 1, false);
    let _ = notes(&r);
    r.turn_start("fg", None, None).unwrap();
    let n = notes(&r);
    assert!(
        n.iter()
            .any(|(_, w, _)| w == "dropped: 1 held chunk(s) (turn_start, read_mode done)"),
        "{n:?}"
    );
    // Nothing held: no note.
    r.turn_start("fg", None, None).unwrap();
    assert!(notes(&r).is_empty());
}

#[test]
fn read_mode_does_not_change_summaries() {
    for mode in [ReadMode::Immediate, ReadMode::Queue, ReadMode::Done] {
        let mut r = summary_rules();
        r.settings.read_mode = mode;
        assert!(spoken(&prose(&mut r, "fg", "Recorded only. ", 0, true)).is_empty());
        let a = r.turn_end("fg", None, None).unwrap();
        assert!(spoken(&a).is_empty(), "{mode:?}: summaries own the turn");
    }
}

#[test]
fn read_mode_names_round_trip() {
    for m in [ReadMode::Immediate, ReadMode::Queue, ReadMode::Done] {
        assert_eq!(ReadMode::parse(m.as_str()), Some(m));
    }
    assert_eq!(ReadMode::parse("later"), None);
    assert_eq!(ReadMode::parse("immediate"), Some(ReadMode::Immediate));
    assert_eq!(ReadMode::parse("queue"), Some(ReadMode::Queue));
    assert_eq!(ReadMode::parse("done"), Some(ReadMode::Done));
}

// -- decisions ---------------------------------------------------------------

#[test]
fn a_question_chimes_and_is_spoken_as_a_decision() {
    let mut r = rules();
    let a = r.ask("fg", &question("Which color?", &["Red", "Blue"]));
    assert_eq!(earcons(&a), [Earcon::Choice]);
    assert_eq!(spoken(&a), ["!Which color? Option 1: Red. Option 2: Blue."]);
    assert!(r.awaiting("fg"));
    // A second question of the same set does not chime again.
    let a = r.ask("fg", &question("And size?", &["S"]));
    assert!(earcons(&a).is_empty());
    assert_eq!(spoken(&a), ["!And size? Option 1: S."]);
}

#[test]
fn decisions_are_spoken_at_every_verbosity() {
    for v in [Verbosity::Everything, Verbosity::SkipCode] {
        let mut r = rules();
        r.settings.verbosity = v;
        assert_eq!(
            spoken(&r.ask("fg", &Ask::new(AskKind::Plan, "Do it."))),
            ["!Plan ready. Do it."],
            "{v:?}"
        );
    }
}

#[test]
fn a_plan_has_no_earcon_and_a_permission_has_its_own() {
    let mut r = rules();
    let a = r.ask("fg", &Ask::new(AskKind::Plan, ""));
    assert!(earcons(&a).is_empty());
    assert_eq!(spoken(&a), ["!A plan is ready for your review."]);
    let a = r.ask("fg", &Ask::new(AskKind::Permission, "Run git status"));
    assert_eq!(earcons(&a), [Earcon::Permission]);
    assert_eq!(spoken(&a), ["!Run git status"]);
}

#[test]
fn the_permission_an_unanswered_question_fires_is_dropped_once() {
    // test_daemon_decision_dedup.py (#11 follow-up)
    let mut r = rules();
    r.ask("fg", &question("Pick?", &["a", "b"]));
    // Its lead-in prose streams after the question: it does not clear the
    // mark.
    prose(&mut r, "fg", "Continuing. ", 5, true);
    let a = r.ask(
        "fg",
        &Ask::new(AskKind::Permission, "Claude needs your permission"),
    );
    assert!(a.is_empty(), "no earcon, no text: {a:?}");
    assert!(!r.awaiting("fg"), "the mark is consumed");
    let a = r.ask("fg", &Ask::new(AskKind::Permission, "Run ls"));
    assert_eq!(earcons(&a), [Earcon::Permission]);
    assert_eq!(spoken(&a), ["!Run ls"]);
}

#[test]
fn a_tool_an_answer_or_a_new_turn_clears_the_question_mark() {
    for clear in ["tool", "answered", "turn_start"] {
        let mut r = rules();
        r.ask("fg", &question("Pick?", &["a"]));
        match clear {
            "tool" => {
                r.tool("fg", "Bash", "ls");
            }
            "answered" => {
                r.answered("fg");
            }
            _ => {
                r.turn_start("fg", None, None).unwrap();
            }
        }
        let a = r.ask("fg", &Ask::new(AskKind::Permission, "Run ls"));
        assert_eq!(spoken(&a), ["!Run ls"], "{clear}");
    }
}

#[test]
fn the_mark_is_per_channel() {
    let mut r = rules();
    r.ask("a", &question("Pick?", &["x"]));
    let a = r.ask("b", &Ask::new(AskKind::Permission, "Run ls"));
    assert_eq!(spoken(&a), ["!Run ls"]);
}

#[test]
fn notes_always_hints_at_everything_and_the_once_hint_once_per_channel() {
    let mut ask = question("Pick?", &["A"]);
    ask.notes = Some("Use arrows.".into());
    ask.hint = Some("Press a number.".into());
    ask.hint_once = Some("Selecting is immediate.".into());
    let mut r = rules();
    assert_eq!(
        spoken(&r.ask("fg", &ask)),
        ["!Pick? Option 1: A. Use arrows. Press a number. Selecting is immediate."]
    );
    r.answered("fg");
    assert_eq!(
        spoken(&r.ask("fg", &ask)),
        ["!Pick? Option 1: A. Use arrows. Press a number."]
    );
    assert_eq!(
        spoken(&r.ask("other", &ask)),
        ["!Pick? Option 1: A. Use arrows. Press a number. Selecting is immediate."]
    );
    let mut r = rules();
    r.settings.verbosity = Verbosity::SkipCode;
    assert_eq!(
        spoken(&r.ask("fg", &ask)),
        ["!Pick? Option 1: A. Use arrows."]
    );
}

#[test]
fn a_decision_follows_the_prose_held_before_it() {
    let mut r = rules();
    r.settings.minqueue = 5;
    prose(&mut r, "fg", "Here is the question. ", 0, false);
    let a = r.ask("fg", &question("Pick?", &["A"]));
    assert_eq!(spoken(&a), ["Here is the question.", "!Pick? Option 1: A."]);
}

#[test]
fn an_answer_wipes_the_channel_without_resuming() {
    let mut r = rules();
    assert_eq!(
        r.answered("fg"),
        [Action::Wipe {
            channel: "fg".into(),
            resume: false
        }]
    );
}

// -- earcons and mute ---------------------------------------------------------

#[test]
fn turn_end_plays_turn_done() {
    let mut r = rules();
    assert_eq!(
        earcons(&r.turn_end("fg", None, None).unwrap()),
        [Earcon::TurnDone]
    );
    assert_eq!(r.play(Earcon::Nav), [Action::Earcon(Earcon::Nav)]);
}

#[test]
fn muted_keeps_beeps_and_super_muted_silences_them() {
    // test_muted_keeps_beeps_super_mute_silences_them
    let mut r = rules();
    assert_eq!(r.set_mute_level(1), [Action::Silence]);
    assert!(spoken(&prose(&mut r, "fg", "Hello. ", 0, true)).is_empty());
    let a = r.ask("fg", &question("Pick?", &["A"]));
    assert_eq!(earcons(&a), [Earcon::Choice]);
    assert!(spoken(&a).is_empty(), "decisions are muted too");
    assert_eq!(r.set_mute_level(2), [Action::Silence]);
    assert!(earcons(&r.turn_end("fg", None, None).unwrap()).is_empty());
    assert!(r.play(Earcon::Nav).is_empty());
    assert!(r.set_mute_level(0).is_empty());
    assert_eq!(spoken(&prose(&mut r, "fg", "Back. ", 0, true)), ["Back."]);
    assert_eq!(r.play(Earcon::Nav), [Action::Earcon(Earcon::Nav)]);
}

// -- summaries ------------------------------------------------------------

/// Turn end in summary mode, then the settle window fires.
fn end_and_settle(r: &mut Rules, ch: &str, focused: Option<&str>) -> Vec<Action> {
    let a = r.turn_end(ch, None, None).unwrap();
    let settle = settle_of(&a);
    let mut out = a;
    out.extend(r.fire(&settle, focused));
    out
}

#[test]
fn summary_mode_records_prose_instead_of_speaking_it() {
    let mut r = summary_rules();
    assert!(prose(&mut r, "fg", "A long explanation. ", 0, true).is_empty());
    let a = r.turn_end("fg", None, None).unwrap();
    assert_eq!(earcons(&a), [Earcon::TurnDone]);
    let (after, timer) = timers(&a).remove(0);
    assert_eq!(after, Duration::from_millis(600));
    assert!(matches!(timer, Timer::Settle { .. }));
    assert!(
        jobs(&a).is_empty(),
        "nothing is summarized before the settle"
    );
}

#[test]
fn a_short_focused_turn_is_spoken_raw_without_a_model_call() {
    let mut r = summary_rules();
    prose(&mut r, "fg", "All done here. Nice. ", 0, true);
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    assert!(jobs(&a).is_empty());
    assert_eq!(spoken(&a), ["All done here.", "Nice."]);
}

#[test]
fn a_short_background_turn_joins_the_summary_order() {
    let mut r = summary_rules();
    prose(&mut r, "bg", "All done here. ", 0, true);
    let a = end_and_settle(&mut r, "bg", Some("fg"));
    assert_eq!(spoken(&a), ["All done here."]);
}

#[test]
fn a_long_turn_is_summarized_and_the_summary_spoken() {
    let mut r = summary_rules();
    prose(
        &mut r,
        "fg",
        &format!("First part. {}", PAD.repeat(3)),
        0,
        true,
    );
    prose(
        &mut r,
        "fg",
        &format!("Second part. {}", PAD.repeat(3)),
        1,
        true,
    );
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    let job = jobs(&a).remove(0);
    assert!(!job.leadin);
    assert!(job.text.contains("First part.") && job.text.contains("Second part."));
    assert!(timers(&a)
        .iter()
        .any(|(d, t)| matches!(t, Timer::Watchdog { .. }) && *d == Duration::from_secs(120)));
    let done = r.digest_done(&job, Some("The **digest** body.".into()));
    assert_eq!(spoken(&done), ["The digest body."], "normalized for speech");
}

#[test]
fn a_failed_turn_end_summary_falls_back_to_the_raw_text() {
    let mut r = summary_rules();
    let long = "This is substantive content the user must hear. ".repeat(8);
    prose(&mut r, "fg", &long, 0, true);
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    let job = jobs(&a).remove(0);
    let done = r.digest_done(&job, None);
    assert_eq!(spoken(&done).len(), 1);
    assert!(spoken(&done)[0].contains("substantive content"));
}

#[test]
fn late_prose_restarts_the_settle_window() {
    let mut r = summary_rules();
    prose(&mut r, "fg", "Part one. ", 0, true);
    let first = settle_of(&r.turn_end("fg", None, None).unwrap());
    let a = prose(&mut r, "fg", "Late part. ", 1, true);
    let second = settle_of(&a);
    assert_ne!(first, second);
    assert!(r.fire(&first, Some("fg")).is_empty(), "stale fire");
    assert_eq!(
        spoken(&r.fire(&second, Some("fg"))),
        ["Part one.", "Late part."]
    );
    assert!(r.fire(&second, Some("fg")).is_empty(), "fires once");
}

#[test]
fn a_turn_end_with_no_prose_dispatches_nothing() {
    let mut r = summary_rules();
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    assert!(jobs(&a).is_empty());
    assert!(spoken(&a).is_empty());
}

#[test]
fn a_decision_in_summary_mode_waits_for_its_lead_in_summary() {
    // #16/#83: even a short lead-in is summarized; the question is held
    // until the summary lands, then spoken after it.
    let mut r = summary_rules();
    prose(&mut r, "fg", "Let me check out this repo. ", 0, true);
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    assert_eq!(earcons(&a), [Earcon::Choice], "the chime is instant");
    assert!(spoken(&a).is_empty(), "the question waits for the settle");
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    assert!(job.leadin);
    assert_eq!(job.seq, None, "lead-ins bypass the order");
    assert!(
        spoken(&fired).is_empty(),
        "nothing raw, the question is held"
    );
    assert_eq!(r.summary_state("fg"), Some((false, 0, 1, 1)));
    let (cap, _) = timers(&fired)
        .into_iter()
        .find(|(_, t)| matches!(t, Timer::HoldCap { .. }))
        .unwrap();
    assert_eq!(cap, Duration::from_secs(65));
    let done = r.digest_done(&job, Some("I checked the repo.".into()));
    assert_eq!(
        spoken(&done),
        ["I checked the repo.", "!Deploy now? Option 1: Yes."]
    );
}

#[test]
fn an_empty_lead_in_summary_is_dropped_and_the_question_still_speaks() {
    let mut r = summary_rules();
    prose(&mut r, "fg", "Let me verify this. ", 0, true);
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    assert_eq!(
        spoken(&r.digest_done(&job, None)),
        ["!Deploy now? Option 1: Yes."]
    );
}

#[test]
fn a_question_with_no_lead_in_is_not_held() {
    let mut r = summary_rules();
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    assert!(jobs(&fired).is_empty());
    assert_eq!(spoken(&fired), ["!Deploy now? Option 1: Yes."]);
}

#[test]
fn the_hold_cap_speaks_the_question_when_the_summary_is_slow() {
    let mut r = summary_rules();
    prose(
        &mut r,
        "fg",
        "Let me check something first before asking. ",
        0,
        true,
    );
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    let cap = timers(&fired)
        .into_iter()
        .map(|(_, t)| t)
        .find(|t| matches!(t, Timer::HoldCap { .. }))
        .unwrap();
    assert_eq!(
        spoken(&r.fire(&cap, Some("fg"))),
        ["!Deploy now? Option 1: Yes."]
    );
    assert!(r.fire(&cap, Some("fg")).is_empty(), "released once");
    // The summary landing later is spoken, the question not again.
    assert_eq!(
        spoken(&r.digest_done(&job, Some("The digest.".into()))),
        ["The digest."]
    );
}

#[test]
fn the_hold_cap_is_a_no_op_after_the_summary_landed() {
    let mut r = summary_rules();
    prose(
        &mut r,
        "fg",
        "Some context before the question arrives. ",
        0,
        true,
    );
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    r.digest_done(&job, Some("The digest.".into()));
    let cap = timers(&fired)
        .into_iter()
        .map(|(_, t)| t)
        .find(|t| matches!(t, Timer::HoldCap { .. }))
        .unwrap();
    assert!(r.fire(&cap, Some("fg")).is_empty());
}

#[test]
fn an_answer_kills_the_lead_in_summary_and_later_summaries_cover_only_what_follows() {
    let mut r = summary_rules();
    prose(
        &mut r,
        "fg",
        &"A long enough lead-in before the question. ".repeat(10),
        0,
        true,
    );
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    r.answered("fg");
    assert_eq!(r.summary_state("fg"), Some((false, 0, 0, 0)));
    assert!(spoken(&r.digest_done(&job, Some("Too late digest.".into()))).is_empty());
    // The assistant goes on after the answer.
    prose(
        &mut r,
        "fg",
        &"Here is what I did after your answer. ".repeat(10),
        0,
        true,
    );
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    let job = jobs(&a).remove(0);
    assert!(job.text.contains("after your answer"));
    assert!(!job.text.contains("lead-in"));
}

#[test]
fn an_answer_drops_a_decision_waiting_for_the_settle() {
    let mut r = summary_rules();
    prose(&mut r, "fg", "Some lead-in. ", 0, true);
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    assert_eq!(r.summary_state("fg"), Some((true, 1, 0, 0)));
    r.answered("fg");
    assert_eq!(r.summary_state("fg"), Some((false, 0, 0, 0)));
    assert!(r.fire(&settle_of(&a), Some("fg")).is_empty());
}

#[test]
fn a_new_turn_drops_the_summary_in_flight_and_held_questions() {
    let mut r = summary_rules();
    prose(&mut r, "fg", &PAD.repeat(6), 0, true);
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    let job = jobs(&a).remove(0);
    r.turn_start("fg", None, None).unwrap();
    assert!(spoken(&r.digest_done(&job, Some("Old digest.".into()))).is_empty());
}

#[test]
fn lead_in_prose_is_not_voiced_again_at_turn_end() {
    let mut r = summary_rules();
    prose(&mut r, "fg", "Short lead-in. ", 0, true);
    let a = r.ask("fg", &question("Deploy now?", &["Yes"]));
    let fired = r.fire(&settle_of(&a), Some("fg"));
    let job = jobs(&fired).remove(0);
    r.digest_done(&job, Some("Lead-in digest.".into()));
    let a = end_and_settle(&mut r, "fg", Some("fg"));
    assert!(jobs(&a).is_empty());
    assert!(spoken(&a).is_empty(), "already voiced: {a:?}");
}

#[test]
fn a_second_turn_end_keeps_the_first_summary() {
    // #13: only the user cancels; a turn merely ending never drops a
    // finished summary.
    let mut r = summary_rules();
    prose(&mut r, "fg", &PAD.repeat(6), 0, true);
    let first = jobs(&end_and_settle(&mut r, "fg", Some("fg"))).remove(0);
    prose(&mut r, "fg", &PAD.repeat(6), 1, true);
    let second = jobs(&end_and_settle(&mut r, "fg", Some("fg"))).remove(0);
    assert_eq!(
        spoken(&r.digest_done(&second, Some("Two.".into()))),
        Vec::<String>::new(),
        "parked behind the first"
    );
    assert_eq!(
        spoken(&r.digest_done(&first, Some("One.".into()))),
        ["One.", "Two."]
    );
}

#[test]
fn summaries_are_heard_in_turn_finish_order_across_channels() {
    // #88: a fast summary for a later turn waits for a slow earlier one.
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    prose(&mut r, "b", &PAD.repeat(6), 0, true);
    let ja = jobs(&end_and_settle(&mut r, "a", None)).remove(0);
    let jb = jobs(&end_and_settle(&mut r, "b", None)).remove(0);
    assert!(spoken(&r.digest_done(&jb, Some("B.".into()))).is_empty());
    let a = r.digest_done(&ja, Some("A.".into()));
    let order: Vec<(String, String)> = a
        .iter()
        .filter_map(|x| match x {
            Action::Speak { channel, text, .. } => Some((channel.clone(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [("a".into(), "A.".into()), ("b".into(), "B.".into())]
    );
}

#[test]
fn the_watchdog_speaks_a_hung_summary_raw_and_releases_the_ones_behind() {
    let mut r = summary_rules();
    prose(
        &mut r,
        "a",
        &format!("Hung turn. {}", PAD.repeat(6)),
        0,
        true,
    );
    prose(&mut r, "b", &PAD.repeat(6), 0, true);
    let ea = end_and_settle(&mut r, "a", None);
    let ja = jobs(&ea).remove(0);
    let jb = jobs(&end_and_settle(&mut r, "b", None)).remove(0);
    assert!(spoken(&r.digest_done(&jb, Some("B.".into()))).is_empty());
    let dog = timers(&ea)
        .into_iter()
        .map(|(_, t)| t)
        .find(|t| matches!(t, Timer::Watchdog { .. }))
        .unwrap();
    let a = r.fire(&dog, None);
    let heard = spoken(&a);
    assert_eq!(heard.len(), 2);
    assert!(heard[0].starts_with("Hung turn."));
    assert_eq!(heard[1], "B.");
    // The hung worker answering at last is ignored.
    assert!(spoken(&r.digest_done(&ja, Some("Late.".into()))).is_empty());
    assert!(r.fire(&dog, None).is_empty());
}

#[test]
fn a_closed_channel_never_comes_back_through_a_late_summary() {
    let mut r = summary_rules();
    prose(&mut r, "fg", &PAD.repeat(6), 0, true);
    let job = jobs(&end_and_settle(&mut r, "fg", Some("fg"))).remove(0);
    r.close("fg");
    assert!(r.channels().is_empty());
    assert!(r.digest_done(&job, Some("Ghost.".into())).is_empty());
    // Reopened under the same id: still not resurrected.
    prose(&mut r, "fg", "Hello. ", 0, true);
    assert!(spoken(&r.digest_done(&job, Some("Ghost.".into()))).is_empty());
}

#[test]
fn stop_drops_summaries_in_flight_settling_and_held() {
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    let job = jobs(&end_and_settle(&mut r, "a", None)).remove(0);
    prose(&mut r, "b", "Lead-in. ", 0, true);
    let settle = settle_of(&r.ask("b", &question("Q?", &[])));
    r.stop_all();
    assert!(r.digest_done(&job, Some("A.".into())).is_empty());
    assert!(r.fire(&settle, None).is_empty());
    assert_eq!(r.summary_state("b"), Some((false, 0, 0, 0)));
}

#[test]
fn a_summary_landing_while_muted_is_not_spoken() {
    let mut r = summary_rules();
    prose(&mut r, "fg", &PAD.repeat(6), 0, true);
    let job = jobs(&end_and_settle(&mut r, "fg", Some("fg"))).remove(0);
    r.set_mute_level(1);
    assert!(spoken(&r.digest_done(&job, Some("Digest.".into()))).is_empty());
}

/// Whether each spoken text is a summary release (read past the
/// background policy).
fn releases(actions: &[Action]) -> Vec<bool> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Speak { release, .. } => Some(*release),
            _ => None,
        })
        .collect()
}

#[test]
fn a_summary_and_its_raw_fallback_are_releases_live_prose_is_not() {
    // #195: pipeline.py authorizes a digest delivery past earcon_only.
    let mut r = rules();
    let a = r.stream("bg", None, "Live prose.", 0, true, None).unwrap();
    assert_eq!(releases(&a), [false]);
    let a = r.ask("bg", &Ask::new(AskKind::Plan, "x"));
    assert_eq!(releases(&a), [false]);
    let mut r = summary_rules();
    let long = "This is substantive content the user must hear. ".repeat(8);
    prose(&mut r, "bg", &long, 0, true);
    let a = end_and_settle(&mut r, "bg", Some("fg"));
    let job = jobs(&a).remove(0);
    assert_eq!(
        releases(&r.digest_done(&job, Some("Recap.".into()))),
        [true]
    );
    prose(&mut r, "bg", &long, 0, true);
    let a = end_and_settle(&mut r, "bg", Some("fg"));
    let job = jobs(&a).remove(0);
    assert_eq!(releases(&r.digest_done(&job, None)), [true]);
}

#[test]
fn closing_a_channel_frees_its_turn_state() {
    let mut r = rules();
    r.turn_start("a", None, Some(5.0)).unwrap();
    assert!(r.tracks("a"));
    r.close("a");
    assert!(!r.tracks("a"));
    assert!(r.stream("a", None, "Again.", 0, true, Some(1.0)).is_ok());
}

// -- notes and kinds for the troubleshooting log (#219) ---------------------

fn kinds(actions: &[Action]) -> Vec<&'static str> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Speak { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

fn notes(r: &Rules) -> Vec<(&'static str, String, Option<String>)> {
    r.take_notes()
        .into_iter()
        .map(|n| (n.kind, n.what, n.text))
        .collect()
}

#[test]
fn every_spoken_text_names_its_kind() {
    let mut r = rules();
    r.settings.verbosity = Verbosity::Everything;
    assert_eq!(
        kinds(&r.stream("fg", None, "Hi.", 0, true, None).unwrap()),
        ["prose"]
    );
    assert_eq!(
        kinds(&r.ask("fg", &question("Pick?", &["A"]))),
        ["question"]
    );
    r.answered("fg");
    assert_eq!(
        kinds(&r.ask("fg", &Ask::new(AskKind::Permission, "Run ls"))),
        ["permission"]
    );
    assert_eq!(
        kinds(&r.ask("fg", &Ask::new(AskKind::Plan, "Do it."))),
        ["plan"]
    );
    assert_eq!(kinds(&r.tool("fg", "Bash", "git status")), ["tool"]);
    assert!(notes(&r).is_empty(), "nothing was held back");
}

#[test]
fn what_is_not_spoken_leaves_a_note_with_the_reason() {
    let mut r = rules();
    r.settings.verbosity = Verbosity::SkipCode;
    // A code block at skip_code.
    let a = prose(&mut r, "fg", "Look:\n```py\nx = 1\n```\n", 0, true);
    assert!(
        spoken(&a).iter().all(|t| !t.contains("code block")),
        "{a:?}"
    );
    let n = notes(&r);
    assert!(
        n.iter()
            .any(|(k, w, t)| *k == "code" && w.contains("skip_code") && t.is_some()),
        "{n:?}"
    );
    // A tool at skip_code.
    assert!(r.tool("fg", "Bash", "ls").is_empty());
    let n = notes(&r);
    assert_eq!(n[0].0, "tool");
    assert!(n[0].1.contains("verbosity skip_code"), "{n:?}");
    assert_eq!(n[0].2.as_deref(), Some("ls"));
    // The permission prompt of an unanswered question (#11).
    r.ask("fg", &question("Pick?", &["A", "B"]));
    let _ = notes(&r);
    assert!(spoken(&r.ask(
        "fg",
        &Ask::new(AskKind::Permission, "Claude needs your permission")
    ))
    .is_empty());
    let n = notes(&r);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].0, "permission");
    assert!(n[0].1.contains("awaiting its answer"), "{n:?}");
    // Muted: stored, not spoken (#243; the driver logs the store).
    r.set_mute_level(1);
    assert_eq!(
        prose(&mut r, "fg", "Quiet please.", 1, true),
        [Action::Store {
            channel: "fg".into(),
            text: "Quiet please.".into(),
            decision: false,
            kind: "prose",
        }]
    );
    r.set_mute_level(2);
    assert!(r.turn_end("fg", None, None).unwrap().is_empty());
    let n = notes(&r);
    assert!(
        n.iter()
            .any(|(k, w, _)| *k == "earcon" && w.contains("turn_done")),
        "{n:?}"
    );
}

#[test]
fn prose_held_below_minqueue_is_noted() {
    let mut r = rules();
    r.settings.minqueue = 3;
    assert!(prose(&mut r, "fg", "One.", 0, true).is_empty());
    let n = notes(&r);
    assert_eq!(n.len(), 1);
    assert!(
        n[0].1.contains("held: 1 chunk(s) wait for minqueue 3"),
        "{n:?}"
    );
}

// -- flush: only the session being read (#228) -----------------------------

fn done_rules() -> Rules {
    Rules::new(Settings {
        read_mode: ReadMode::Done,
        ..Settings::default()
    })
}

/// The notes of `channel` as `kind: what` lines.
fn notes_of(r: &Rules, channel: &str) -> Vec<String> {
    r.take_notes()
        .into_iter()
        .filter(|n| n.channel.as_deref() == Some(channel))
        .map(|n| format!("{}: {}", n.kind, n.what))
        .collect()
}

#[test]
fn flush_while_another_session_streams_keeps_its_message() {
    // #228: flushing the session being read wiped the prose another
    // session was still streaming (summary mode); only its last sentences,
    // arriving after the flush, were heard.
    let mut r = summary_rules();
    prose(&mut r, "wind", &PAD.repeat(6), 0, false);
    prose(&mut r, "dl", "The downloads recap. ", 0, true);
    r.flush("dl");
    prose(&mut r, "wind", "Last words. ", 1, true);
    let job = jobs(&end_and_settle(&mut r, "wind", None)).remove(0);
    assert_eq!(job.text.matches("filler").count(), 6, "{}", job.text);
    assert!(job.text.ends_with("Last words."), "{}", job.text);
}

#[test]
fn flush_stops_only_the_session_being_read() {
    let mut r = done_rules();
    prose(&mut r, "a", "Alpha one. ", 0, true);
    prose(&mut r, "b", "Beta one. ", 0, true);
    r.flush("a");
    assert_eq!(spoken(&r.turn_end("b", None, None).unwrap()), ["Beta one."]);
    assert!(spoken(&r.turn_end("a", None, None).unwrap()).is_empty());
}

#[test]
fn flush_keeps_other_sessions_summaries_and_decisions() {
    let mut r = summary_rules();
    prose(&mut r, "b", &PAD.repeat(6), 0, true);
    let job = jobs(&end_and_settle(&mut r, "b", None)).remove(0);
    prose(&mut r, "c", "Lead-in. ", 0, true);
    r.ask("c", &question("Deploy?", &[]));
    prose(&mut r, "a", "Alpha. ", 0, true);
    r.flush("a");
    assert_eq!(r.summary_state("b"), Some((false, 0, 0, 1)));
    assert_eq!(r.summary_state("c"), Some((true, 1, 0, 0)));
    assert_eq!(spoken(&r.digest_done(&job, Some("B.".into()))), ["B."]);
}

#[test]
fn the_rest_of_a_flushed_reply_is_skipped() {
    // #228 (flush_scope, 2026-10-04): the flushed session is skipped for
    // the whole reply, also the prose that arrives after the flush.
    for mode in [ReadMode::Immediate, ReadMode::Queue, ReadMode::Done] {
        let mut r = Rules::new(Settings {
            read_mode: mode,
            ..Settings::default()
        });
        prose(&mut r, "a", "Heard. ", 0, true);
        r.flush("a");
        r.take_notes();
        assert!(spoken(&prose(&mut r, "a", "After. ", 1, true)).is_empty());
        assert_eq!(
            notes_of(&r, "a"),
            ["prose: dropped: flushed reply"],
            "{mode:?}"
        );
        let end = r.turn_end("a", None, None).unwrap();
        assert!(spoken(&end).is_empty(), "{mode:?}");
        assert_eq!(earcons(&end), [Earcon::TurnDone], "the reply still ends");
    }
}

#[test]
fn the_flushed_reply_is_skipped_until_the_sessions_next_turn_start() {
    let mut r = done_rules();
    prose(&mut r, "a", "Heard. ", 0, true);
    r.flush("a");
    // An answer does not end the reply.
    r.answered("a");
    assert!(spoken(&prose(&mut r, "a", "Still skipped. ", 1, true)).is_empty());
    r.tool("a", "Bash", "");
    r.turn_end("a", None, None).unwrap();
    // Late prose of the flushed reply, after its turn_end (#14).
    assert!(spoken(&prose(&mut r, "a", "Late. ", 2, true)).is_empty());
    r.turn_start("a", None, None).unwrap();
    prose(&mut r, "a", "Next reply. ", 0, true);
    assert_eq!(
        spoken(&r.turn_end("a", None, None).unwrap()),
        ["Next reply."]
    );
}

#[test]
fn a_tool_in_the_flushed_reply_is_not_announced() {
    let mut r = rules();
    r.settings.verbosity = Verbosity::Everything;
    prose(&mut r, "a", "Heard. ", 0, true);
    r.flush("a");
    r.take_notes();
    assert!(spoken(&r.tool("a", "Bash", "Running the tests.")).is_empty());
    assert_eq!(notes_of(&r, "a"), ["tool: not announced: flushed reply"]);
}

#[test]
fn a_question_later_in_the_flushed_reply_is_still_spoken() {
    // #228: a decision needs an answer, so it is read even though the rest
    // of its reply is skipped.
    let mut r = done_rules();
    prose(&mut r, "a", "Heard. ", 0, true);
    r.flush("a");
    prose(&mut r, "a", "Lead-in skipped. ", 1, true);
    let out = r.ask("a", &question("Deploy?", &["Yes", "No"]));
    let said = spoken(&out);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with('!') && said[0].contains("Deploy?"),
        "{said:?}"
    );
    assert_eq!(earcons(&out), [Earcon::Choice]);
    let perm = r.ask("a", &Ask::new(AskKind::Plan, "The plan."));
    assert_eq!(spoken(&perm).len(), 1, "a plan too");
}

#[test]
fn a_question_later_in_a_flushed_summary_reply_is_still_spoken() {
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    r.flush("a");
    prose(&mut r, "a", &PAD.repeat(6), 1, true);
    let asked = r.ask("a", &question("Ship?", &[]));
    let out = r.fire(&settle_of(&asked), None);
    assert!(
        jobs(&out).is_empty(),
        "nothing of the flushed reply to recap"
    );
    let said = spoken(&out);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("Ship?"), "{said:?}");
}

#[test]
fn a_flushed_summary_reply_makes_no_summary_and_the_next_turn_does() {
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    r.flush("a");
    prose(&mut r, "a", "After the flush. ", 1, true);
    let a = end_and_settle(&mut r, "a", Some("a"));
    assert!(jobs(&a).is_empty());
    assert!(spoken(&a).is_empty(), "{:?}", spoken(&a));
    r.turn_start("a", None, None).unwrap();
    prose(&mut r, "a", "A new reply. ", 0, true);
    assert_eq!(
        spoken(&end_and_settle(&mut r, "a", Some("a"))),
        ["A new reply."]
    );
}

// -- flush_scope all: every ready message (#228) ---------------------------

#[test]
fn flush_ready_drops_a_finished_turns_work_and_keeps_a_turn_still_arriving() {
    let mut r = summary_rules();
    // b finished its turn: its summary is in flight.
    prose(&mut r, "b", &PAD.repeat(6), 0, true);
    let job = jobs(&end_and_settle(&mut r, "b", None)).remove(0);
    // c is still writing.
    prose(&mut r, "c", &PAD.repeat(6), 0, false);
    assert!(!r.writing("b"));
    assert!(r.writing("c"));
    r.take_notes();
    assert!(r.flush_ready("b", false).is_some());
    assert!(
        r.flush_ready("c", true).is_none(),
        "a turn still arriving stays"
    );
    assert_eq!(
        notes_of(&r, "b"),
        ["summary: cancelled (flush): 1 summary in flight"]
    );
    assert!(spoken(&r.digest_done(&job, Some("B.".into()))).is_empty());
    // b is not skipped for later: only the session being read is.
    prose(&mut r, "c", "Last words. ", 1, true);
    let c = jobs(&end_and_settle(&mut r, "c", None)).remove(0);
    assert_eq!(c.text.matches("filler").count(), 6, "{}", c.text);
}

#[test]
fn flush_ready_with_nothing_to_drop_says_so() {
    let mut r = done_rules();
    prose(&mut r, "b", "Done. ", 0, true);
    r.turn_end("b", None, None).unwrap();
    assert!(r.flush_ready("b", false).is_none());
    assert!(r.flush_ready("ghost", true).is_none());
    // Nothing was flushed: its late prose is still read.
    assert_eq!(spoken(&prose(&mut r, "b", "Late. ", 1, true)), ["Late."]);
}

#[test]
fn flush_all_skips_the_late_prose_of_another_sessions_flushed_reply() {
    // Review of #228: b's reply ended and its text was queued in L2 (the
    // driver says so); late prose of that reply (#14) is skipped too.
    let mut r = rules();
    prose(&mut r, "b", "Read. ", 0, true);
    r.turn_end("b", None, None).unwrap();
    r.take_notes();
    assert!(r.flush_ready("b", true).is_some());
    assert!(spoken(&prose(&mut r, "b", "Late. ", 1, true)).is_empty());
    assert_eq!(notes_of(&r, "b"), ["prose: dropped: flushed reply"]);
    // Its next reply is read as usual.
    r.turn_start("b", None, None).unwrap();
    assert_eq!(
        spoken(&prose(&mut r, "b", "Next reply. ", 0, true)),
        ["Next reply."]
    );
}

#[test]
fn flush_all_skips_late_prose_after_cancelling_a_summary() {
    // Summaries on: the settle window was cancelled, so late prose would be
    // kept and never summarized; it is dropped instead.
    let mut r = summary_rules();
    prose(&mut r, "b", &PAD.repeat(6), 0, true);
    let a = r.turn_end("b", None, None).unwrap();
    let settle = settle_of(&a);
    assert!(r.flush_ready("b", false).is_some());
    r.take_notes();
    assert!(spoken(&prose(&mut r, "b", "Late. ", 1, true)).is_empty());
    assert_eq!(notes_of(&r, "b"), ["prose: dropped: flushed reply"]);
    assert!(jobs(&r.fire(&settle, None)).is_empty());
}

#[test]
fn a_session_without_a_turn_end_is_still_writing() {
    let mut r = done_rules();
    assert!(!r.writing("ghost"));
    r.turn_start("a", None, None).unwrap();
    assert!(r.writing("a"));
    r.turn_end("a", None, None).unwrap();
    assert!(!r.writing("a"));
    r.turn_start("a", None, None).unwrap();
    assert!(r.writing("a"));
}

#[test]
fn flush_drop_is_logged() {
    // #228: what a flush drops leaves a note with the session, the count
    // and the reason (it was silent).
    let mut r = done_rules();
    prose(&mut r, "a", "One. Two. ", 0, true);
    r.take_notes();
    r.flush("a");
    assert_eq!(
        notes_of(&r, "a"),
        ["prose: dropped: 2 held chunk(s) (flush, read_mode done)"]
    );

    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    end_and_settle(&mut r, "a", None);
    r.ask("a", &question("Deploy?", &[]));
    prose(&mut r, "a", "More text. ", 1, true);
    r.take_notes();
    r.flush("a");
    let notes = notes_of(&r, "a");
    assert_eq!(
        notes,
        [
            "prose: dropped: 1 chunk(s) kept for the summary (flush)",
            "summary: cancelled (flush): 1 summary in flight, the settle window",
            "question: spoken now: the summary it waited for was flushed",
        ],
        "{notes:?}"
    );
}

#[test]
fn stop_drop_is_logged_for_every_session() {
    let mut r = done_rules();
    prose(&mut r, "a", "One. ", 0, true);
    prose(&mut r, "b", "Two. Three. ", 0, true);
    r.take_notes();
    r.stop_all();
    let notes: Vec<String> = r
        .take_notes()
        .into_iter()
        .map(|n| format!("{} {}", n.channel.unwrap_or_default(), n.what))
        .collect();
    assert_eq!(
        notes,
        [
            "a dropped: 1 held chunk(s) (stop, read_mode done)",
            "b dropped: 2 held chunk(s) (stop, read_mode done)",
        ]
    );
}

#[test]
fn flush_of_a_session_without_turn_state_does_nothing() {
    let mut r = rules();
    r.flush("ghost");
    assert!(r.channels().is_empty());
    assert!(r.take_notes().is_empty());
}

#[test]
fn flush_of_a_question_keeps_its_permission_prompt_silent() {
    // #228 review: flushing the question being read does not answer it, so
    // the permission prompt the same question fires stays suppressed (#11).
    let mut r = rules();
    r.ask("a", &question("Deploy?", &[]));
    r.flush("a");
    assert!(r.awaiting("a"));
    let out = r.ask(
        "a",
        &Ask::new(AskKind::Permission, "Claude needs your permission"),
    );
    assert!(spoken(&out).is_empty(), "{:?}", spoken(&out));
    assert!(earcons(&out).is_empty());
}

#[test]
fn flush_speaks_the_decisions_that_waited_for_the_summary() {
    // #228 review: skipping a session's recap must not lose its question.
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    end_and_settle(&mut r, "a", None);
    r.ask("a", &question("Deploy?", &[]));
    r.take_notes();
    let out = r.flush("a");
    let said = spoken(&out);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with('!') && said[0].contains("Deploy?"),
        "{said:?}"
    );
    assert!(notes_of(&r, "a")
        .contains(&"question: spoken now: the summary it waited for was flushed".to_string()));

    // Held behind the summary in flight (after the settle window).
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    end_and_settle(&mut r, "a", None);
    let asked = r.ask("a", &question("Ship?", &[]));
    r.fire(&settle_of(&asked), None);
    assert_eq!(r.summary_state("a").map(|s| s.2), Some(1), "held");
    let said = spoken(&r.flush("a"));
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("Ship?"), "{said:?}");
}

#[test]
fn stop_still_drops_the_decisions_that_waited_for_the_summary() {
    let mut r = summary_rules();
    prose(&mut r, "a", &PAD.repeat(6), 0, true);
    end_and_settle(&mut r, "a", None);
    r.ask("a", &question("Deploy?", &[]));
    r.take_notes();
    r.stop_all();
    assert!(
        notes_of(&r, "a").contains(&"question: dropped: waited for the summary (stop)".to_string())
    );
    assert!(!r.awaiting("a"));
}

// -- question sets (#283) ---------------------------------------------------

/// A set of `n` questions as the hook sends it: `set` on each, the hints
/// and notes on the first.
fn question_set(n: usize) -> Vec<Ask> {
    (0..n)
        .map(|i| {
            let mut a = question(&format!("Q{}?", i + 1), &["Yes"]);
            a.set = Some((i, n));
            if i == 0 {
                a.notes = Some("Use arrows.".into());
                a.hint = Some("Press a number.".into());
                a.hint_once = Some("Selecting is immediate.".into());
            }
            a
        })
        .collect()
}

/// Ask every question of a set; the actions of all of them.
fn ask_set(r: &mut Rules, ch: &str, n: usize) -> Vec<Action> {
    question_set(n).iter().flat_map(|a| r.ask(ch, a)).collect()
}

#[test]
fn a_question_set_reads_only_its_first_question_with_question_1_of_n() {
    // Bug 1 of #283: the 4 questions were read back to back.
    let mut r = rules();
    let a = ask_set(&mut r, "fg", 4);
    assert_eq!(
        spoken(&a),
        ["!Question 1 of 4. Q1? Option 1: Yes. Use arrows. Press a number. Selecting is immediate."]
    );
    assert_eq!(earcons(&a), [Earcon::Choice], "one chime for the set");
    assert!(r.awaiting("fg"));
}

#[test]
fn the_other_questions_of_a_set_are_kept_not_spoken() {
    let mut r = rules();
    let set = question_set(3);
    r.ask("fg", &set[0]);
    r.take_notes();
    assert!(spoken(&r.ask("fg", &set[1])).is_empty());
    let notes = r.take_notes();
    assert!(
        notes
            .iter()
            .any(|n| n.what == "kept for navigation: question 2 of 3 (#283)"),
        "{notes:?}"
    );
    assert_eq!(r.question_set("fg"), Some((0, 3)));
}

#[test]
fn a_single_question_has_no_question_1_of_1_prefix() {
    let mut r = rules();
    let mut one = question("Pick?", &["A"]);
    one.set = Some((0, 1));
    assert_eq!(
        spoken(&r.ask("fg", &one)),
        ["!Pick? Option 1: A."],
        "a set of one is a plain question"
    );
    assert_eq!(r.question_set("fg"), None);
    // Without the set fields, every question is read as before.
    let mut r = rules();
    let a: Vec<Action> = ["One?", "Two?"]
        .iter()
        .flat_map(|q| r.ask("fg", &question(q, &[])))
        .collect();
    assert_eq!(spoken(&a), ["!One?", "!Two?"]);
}

#[test]
fn navigate_from_the_text_goes_to_question_one() {
    for step in [Step::Next, Step::Previous] {
        let mut r = rules();
        ask_set(&mut r, "fg", 3);
        r.bind_question("fg", 7, false);
        let nav = r.navigate("fg", step, false).expect("a set");
        assert_eq!((nav.entry, nav.index, nav.size, nav.edge), (7, 0, 3, false));
        assert_eq!(
            nav.text,
            "Question 1 of 3. Q1? Option 1: Yes. Use arrows. Press a number."
        );
    }
}

#[test]
fn next_question_moves_on_and_edges_on_the_last() {
    let mut r = rules();
    ask_set(&mut r, "fg", 3);
    // No entry yet (nothing spoken): nothing to navigate.
    assert_eq!(r.navigate("fg", Step::Next, true), None);
    r.bind_question("fg", 7, false);
    let nav = r.navigate("fg", Step::Next, true).unwrap();
    assert_eq!((nav.index, nav.edge), (1, false));
    assert_eq!(
        nav.text,
        "Question 2 of 3. Q2? Option 1: Yes. Use arrows. Press a number."
    );
    assert_eq!(r.navigate("fg", Step::Next, true).unwrap().index, 2);
    let edge = r.navigate("fg", Step::Next, true).unwrap();
    assert_eq!((edge.index, edge.edge), (2, true), "the last one stays");
    assert_eq!(r.question_set("fg"), Some((2, 3)));
    assert_eq!(r.navigate("other", Step::Next, true), None);
}

#[test]
fn previous_question_on_question_one_restarts_it() {
    let mut r = rules();
    ask_set(&mut r, "fg", 2);
    r.bind_question("fg", 7, false);
    r.navigate("fg", Step::Next, true);
    assert_eq!(r.navigate("fg", Step::Previous, true).unwrap().index, 0);
    let again = r.navigate("fg", Step::Previous, true).unwrap();
    assert_eq!((again.index, again.edge), (0, false), "question 1 again");
    // Up: question 1, whichever question was current.
    r.navigate("fg", Step::Next, true);
    assert_eq!(r.rewind_questions("fg").unwrap().index, 0);
    assert_eq!(r.question_set("fg"), Some((0, 2)));
}

#[test]
fn answered_and_turn_start_forget_the_question_set() {
    let mut r = rules();
    ask_set(&mut r, "fg", 2);
    r.answered("fg");
    assert_eq!(r.question_set("fg"), None);
    ask_set(&mut r, "fg", 2);
    r.turn_start("fg", None, None).unwrap();
    assert_eq!(r.question_set("fg"), None);
    ask_set(&mut r, "fg", 2);
    r.tool("fg", "Bash", "ls");
    assert_eq!(r.question_set("fg"), None);
    // A flush keeps it: the user can still navigate after Down.
    ask_set(&mut r, "fg", 2);
    r.flush("fg");
    assert_eq!(r.question_set("fg"), Some((0, 2)));
    // A plain question replaces it.
    r.ask("fg", &question("Other?", &[]));
    assert_eq!(r.question_set("fg"), None);
}

#[test]
fn the_set_hint_and_notes_are_read_with_each_question_and_hint_once_once() {
    let mut r = rules();
    let a = ask_set(&mut r, "fg", 2);
    assert!(spoken(&a)[0].ends_with("Use arrows. Press a number. Selecting is immediate."));
    r.bind_question("fg", 7, false);
    let q2 = r.navigate("fg", Step::Next, true).unwrap().text;
    assert!(
        q2.ends_with("Q2? Option 1: Yes. Use arrows. Press a number."),
        "{q2}"
    );
    // At verbosity skip_code: the notes alone.
    let mut r = rules();
    r.settings.verbosity = Verbosity::SkipCode;
    let a = ask_set(&mut r, "fg", 2);
    assert_eq!(
        spoken(&a),
        ["!Question 1 of 2. Q1? Option 1: Yes. Use arrows."]
    );
}
