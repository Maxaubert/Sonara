//! The pure L3 rules, ported from the Python daemon's behaviour tests
//! (`test_daemon_prose.py`, `test_daemon_flush_late_prose.py`,
//! `test_daemon_minqueue.py`, `test_daemon_decisions.py`,
//! `test_daemon_decision_dedup.py`, `test_daemon_pause_mute.py`,
//! `test_daemon_question_flow.py`, `test_daemon_summary_mode.py`,
//! `test_summary_pipeline.py`, `test_digest_reorder.py`). Issue numbers name
//! the Python regressions.
use sonara_agent::settings::Settings;
use sonara_agent::{Action, Ask, AskKind, Choice, Earcon, Job, Rules, Stale, Timer, Verbosity};
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
    // Muted.
    r.set_mute_level(1);
    assert!(prose(&mut r, "fg", "Quiet please.", 1, true).is_empty());
    let n = notes(&r);
    assert!(
        n.iter().any(|(k, w, t)| *k == "prose"
            && w == "not spoken: mute level 1"
            && t.as_deref() == Some("Quiet please.")),
        "{n:?}"
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
