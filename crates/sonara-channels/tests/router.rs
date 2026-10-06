//! The pure router, ported from the Python plugin's `tests/test_router.py`
//! and `tests/test_channel.py` (the parts that are not agent features), plus
//! the L2 policies. Issue numbers name the Python regressions they guard.
use sonara_channels::{Feed, Policy, Resolved, Router};

/// One feed in compact notation: an entry's text, or `[label]` for an
/// announcement (`[]` for a channel without a label, `again` for a replay,
/// `manual` for `next_channel`).
fn show(f: Option<Feed>) -> Option<String> {
    f.map(|f| match f {
        Feed::Entry { entry, .. } => entry.text,
        Feed::Announce {
            label,
            replay,
            manual,
            ..
        } => format!(
            "[{}{}{}]",
            label.unwrap_or_default(),
            if replay { " again" } else { "" },
            if manual { " manual" } else { "" }
        ),
    })
}

fn next(r: &mut Router) -> Option<String> {
    show(r.next_feed())
}

/// Everything left to read, in order.
fn drain(r: &mut Router) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(s) = next(r) {
        out.push(s);
    }
    out
}

/// A router with queue channels labelled by their lower-case id.
fn router(ids: &[&str]) -> Router {
    let mut r = Router::new();
    for id in ids {
        r.open(id, Some(id.to_lowercase()), None, Some(Policy::Queue));
    }
    r
}

fn push(r: &mut Router, id: &str, texts: &[&str]) {
    for t in texts {
        r.push(id, t, None).unwrap();
    }
}

fn cursor(r: &Router, id: &str) -> usize {
    r.channel(id).unwrap().cursor()
}

#[test]
fn single_channel_reads_in_order_without_announcement() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one", "two"]);
    assert_eq!(drain(&mut r), ["one", "two"]);
}

#[test]
fn auto_handoff_announces_then_reads_the_focused_channel_first() {
    let mut r = router(&["A", "B", "C"]);
    r.focus("A");
    push(&mut r, "A", &["a1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "C", &["c1"]);
    push(&mut r, "B", &["b1"]);
    r.focus("C");
    // A is caught up: hand off to the focused C before B (opening order).
    assert_eq!(drain(&mut r), ["[c]", "c1", "[b]", "b1"]);
}

#[test]
fn background_channels_are_read_in_opening_order() {
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "C", &["c1"]);
    push(&mut r, "B", &["b1"]);
    // No focus: the first channel with something unread, then the next.
    assert_eq!(drain(&mut r), ["b1", "[c]", "c1"]);
}

#[test]
fn the_reading_channel_finishes_its_batch_before_a_handoff() {
    let mut r = router(&["A", "B"]);
    r.focus("A");
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.focus("B");
    push(&mut r, "B", &["b1"]);
    // A keeps the floor until its batch drains (cooperative).
    assert_eq!(drain(&mut r), ["a2", "[b]", "b1"]);
}

#[test]
fn a_channel_without_a_label_is_announced_without_a_name() {
    // #241: an unnamed switch is still a switch; the driver says it without
    // a name (or not at all when it has no text for it).
    let mut r = Router::new();
    r.open("A", Some("alpha".into()), None, Some(Policy::Queue));
    r.open("B", None, None, Some(Policy::Queue));
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "[]", "b1"]);
    push(&mut r, "A", &["a2"]);
    assert_eq!(drain(&mut r), ["[alpha]", "a2"]);
}

#[test]
fn a_channel_first_created_by_a_stream_is_announced_with_its_label() {
    // #241: after a runtime restart a session's first message is often a
    // stream, which opens its channel without a label; a later message
    // names it, and the switch to it says the name.
    let mut r = router(&["A"]);
    push(&mut r, "A", &["a1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.open("B", None, None, Some(Policy::Queue));
    push(&mut r, "B", &["b1"]);
    assert!(r.label_if_missing("B", "beta"));
    assert!(
        !r.label_if_missing("B", "other"),
        "a label is never replaced"
    );
    assert!(!r.label_if_missing("A", "other"));
    assert!(!r.label_if_missing("Z", "zeta"), "not open");
    assert_eq!(drain(&mut r), ["[beta]", "b1"]);
    assert_eq!(r.channel("B").unwrap().label.as_deref(), Some("beta"));
}

#[test]
fn a_switch_after_the_last_reader_closed_is_announced() {
    // #241, the log of 2026-10-04 23:29:55: "agent-hooks" read, its session
    // ended (channel_close), and a question in "work" was read with no
    // announcement because closing forgot who read last.
    let mut r = router(&["hooks", "work"]);
    push(&mut r, "hooks", &["ok"]);
    assert_eq!(drain(&mut r), ["ok"]);
    r.close("hooks");
    push(&mut r, "work", &["Red or blue?"]);
    assert!(r.prioritize("work"));
    assert_eq!(drain(&mut r), ["[work]", "Red or blue?"]);
}

#[test]
fn a_new_session_in_the_closed_readers_host_tab_is_not_announced() {
    // A /clear (or exit and relaunch) in the same host tab replaces the
    // session that read last: the same place, not a switch.
    let mut r = Router::new();
    r.open(
        "old",
        Some("repo".into()),
        Some("3".into()),
        Some(Policy::Queue),
    );
    push(&mut r, "old", &["bye"]);
    assert_eq!(drain(&mut r), ["bye"]);
    r.close("old");
    r.open(
        "new",
        Some("repo".into()),
        Some("3".into()),
        Some(Policy::Queue),
    );
    push(&mut r, "new", &["hello"]);
    assert_eq!(drain(&mut r), ["hello"]);
    // Another tab after that is a switch again.
    r.open(
        "other",
        Some("work".into()),
        Some("4".into()),
        Some(Policy::Queue),
    );
    push(&mut r, "other", &["hi"]);
    assert_eq!(drain(&mut r), ["[work]", "hi"]);
}

#[test]
fn a_new_session_in_another_host_tab_after_a_close_is_announced() {
    let mut r = Router::new();
    r.open(
        "old",
        Some("repo".into()),
        Some("3".into()),
        Some(Policy::Queue),
    );
    push(&mut r, "old", &["bye"]);
    assert_eq!(drain(&mut r), ["bye"]);
    r.close("old");
    r.open(
        "new",
        Some("repo".into()),
        Some("5".into()),
        Some(Policy::Queue),
    );
    push(&mut r, "new", &["hello"]);
    assert_eq!(drain(&mut r), ["[repo]", "hello"]);
}

#[test]
fn a_prioritized_question_in_another_channel_is_announced_once() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1"]);
    assert_eq!(drain(&mut r), ["a1"]);
    push(&mut r, "B", &["question"]);
    assert!(r.prioritize("B"));
    assert_eq!(drain(&mut r), ["[b]", "question"]);
    push(&mut r, "B", &["more"]);
    assert_eq!(
        drain(&mut r),
        ["more"],
        "the same channel again: no announcement"
    );
}

#[test]
fn the_same_channel_reopened_after_closing_is_not_announced() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["a1"]);
    assert_eq!(drain(&mut r), ["a1"]);
    r.close("A");
    r.open("A", Some("a".into()), None, Some(Policy::Queue));
    push(&mut r, "A", &["a2"]);
    assert_eq!(drain(&mut r), ["a2"]);
}

#[test]
fn next_channel_advances_one_slot_in_a_fixed_order() {
    let mut r = router(&["A", "B", "C"]);
    for id in ["A", "B", "C"] {
        push(&mut r, id, &[&id.to_lowercase()]);
    }
    assert_eq!(next(&mut r).unwrap(), "a"); // A reads
    assert_eq!(r.next_channel().unwrap().0, "B");
    assert_eq!(r.next_channel().unwrap().0, "C");
    assert_eq!(r.next_channel().unwrap().0, "A"); // wraps
}

#[test]
fn switched_away_channel_resumes_after_new_content() {
    // #115: suppression is keyed on the content generation, so a batch
    // that lands back on the same length still lifts it.
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["digest one"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "digest one");
    r.next_channel(); // force-switch A -> B
    assert!(r.is_suppressed("A"));
    push(&mut r, "A", &["digest two"]); // a new batch of the same length
    assert!(!r.is_suppressed("A"));
    assert!(drain(&mut r).contains(&"digest two".to_string()));
}

#[test]
fn next_channel_skips_channels_with_nothing_to_hear() {
    // #117: landing on an empty channel announced the switch, then fell
    // through to an auto hand-off: two announcements for one press.
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "C", &["c1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    assert_eq!(r.next_channel().unwrap().0, "C");
    assert_eq!(r.next_channel().unwrap().0, "A");
}

#[test]
fn next_channel_degrades_to_the_full_ring_when_all_are_empty() {
    let mut r = router(&["A", "B"]);
    let (target, _) = r.next_channel().unwrap();
    assert!(target == "A" || target == "B");
}

#[test]
fn next_channel_continues_the_ring_after_an_idle_gap() {
    // #111: the ring continues from the channel that read last, not from
    // the first channel, once the reader went idle.
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "B", &["b"]);
    assert_eq!(drain(&mut r), ["b"]); // B read, then idle
    assert_eq!(r.active(), None);
    assert_eq!(r.last_active(), Some("B"));
    push(&mut r, "A", &["a"]);
    push(&mut r, "C", &["c"]);
    r.flush(None);
    assert_eq!(r.next_channel().unwrap().0, "C");
}

#[test]
fn next_channel_starts_at_the_first_channel_when_the_last_reader_closed() {
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "B", &["b"]);
    assert_eq!(drain(&mut r), ["b"]);
    r.close("B");
    r.open("B", Some("b".into()), None, Some(Policy::Queue)); // reopened empty, now last
    push(&mut r, "C", &["c"]);
    // The last reader is gone; the ring (C, skipping the empty A and B)
    // starts at its first member.
    assert_eq!(r.next_channel().unwrap().0, "C");
}

#[test]
fn next_channel_arms_a_manual_announcement() {
    let mut r = router(&["A", "B"]);
    r.focus("A");
    push(&mut r, "A", &["a1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["b1"]);
    r.next_channel();
    assert_eq!(next(&mut r).unwrap(), "[b manual]");
    assert_eq!(next(&mut r).unwrap(), "b1");
    // An auto hand-off back to a refilled A is not manual.
    push(&mut r, "A", &["a2"]);
    assert_eq!(drain(&mut r), ["[a]", "a2"]);
}

#[test]
fn next_channel_resumes_an_unread_target() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "B", &["b1", "b2"]);
    assert_eq!(next(&mut r).unwrap(), "b1"); // B half read
    push(&mut r, "A", &["a1"]);
    r.focus("A");
    r.next_channel(); // B -> A
    assert_eq!(drain(&mut r), ["[a manual]", "a1"]);
    assert_eq!(r.next_channel(), Some(("B".to_string(), false)));
    assert_eq!(cursor(&r, "B"), 1); // not reset
    assert_eq!(drain(&mut r), ["[b manual]", "b2"]);
}

#[test]
fn next_channel_replays_a_fully_heard_target() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "a2", "[b]", "b1"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(cursor(&r, "A"), 0);
    assert_eq!(drain(&mut r), ["[a again manual]", "a1", "a2"]);
}

#[test]
fn relanding_on_a_half_played_replay_restarts_it() {
    // #118: re-landing on a replay that was cut halfway resumed mid-message
    // without "reading again".
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "a2", "[b]", "b1"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(next(&mut r).unwrap(), "[a again manual]");
    assert_eq!(next(&mut r).unwrap(), "a1"); // the replay is half played
    assert_eq!(r.next_channel(), Some(("B".to_string(), true)));
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(cursor(&r, "A"), 0); // restarted from the top
}

#[test]
fn new_content_after_a_cut_replay_resumes() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "[b]", "b1"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(next(&mut r).unwrap(), "[a again manual]");
    push(&mut r, "A", &["a2"]); // new content lands mid-replay
    assert_eq!(r.next_channel(), Some(("B".to_string(), true)));
    assert_eq!(r.next_channel(), Some(("A".to_string(), false)));
}

#[test]
fn landing_on_yourself_mid_read_replays() {
    // #118: a single-member ring lands on itself and replays from the top.
    let mut r = router(&["A"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(cursor(&r, "A"), 0);
}

#[test]
fn next_channel_is_none_without_channels() {
    let mut r = Router::new();
    assert_eq!(r.next_channel(), None);
}

#[test]
fn closing_a_channel_clears_its_armed_replay_announcement() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "[b]", "b1"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    r.close("A");
    assert!(!r.announce_armed());
    push(&mut r, "B", &["b2"]);
    // The user switched away from B, so its new text is announced (#241),
    // with no "again": it is new content.
    assert_eq!(drain(&mut r), ["[b]", "b2"]);
}

#[test]
fn a_switched_away_channel_is_not_auto_resumed() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "a1"); // A reading, a2 pending
    assert_eq!(r.next_channel().unwrap().0, "B");
    assert!(r.is_suppressed("A"));
    assert_eq!(drain(&mut r), ["[b manual]", "b1"]); // A stays silent
    assert_eq!(r.pick(), None);
}

#[test]
fn new_content_lifts_the_suppression() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.next_channel();
    drain(&mut r);
    push(&mut r, "A", &["a3"]);
    assert!(!r.is_suppressed("A"));
    assert_eq!(drain(&mut r), ["[a]", "a2", "a3"]);
}

#[test]
fn a_manual_return_clears_the_suppression_and_resumes() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.next_channel(); // A -> B
    assert!(r.is_suppressed("A"));
    assert_eq!(r.next_channel(), Some(("A".to_string(), false)));
    assert!(!r.is_suppressed("A"));
    assert_eq!(cursor(&r, "A"), 1);
}

#[test]
fn clear_announce_drops_an_armed_switch() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    r.next_channel();
    assert!(r.announce_armed());
    r.clear_announce();
    assert!(!r.announce_armed());
    assert_eq!(drain(&mut r), ["a1", "[b]", "b1"]);
}

// --- L2 policies and controls ---

#[test]
fn latest_policy_replaces_the_unread_entries() {
    let mut r = Router::new();
    r.open("A", None, None, None); // latest is the default
    assert_eq!(r.channel("A").unwrap().policy, Policy::Latest);
    push(&mut r, "A", &["one", "two", "three"]);
    assert_eq!(r.channel("A").unwrap().pending(), 1);
    assert_eq!(drain(&mut r), ["three"]);
}

#[test]
fn latest_policy_never_drops_the_latest_message() {
    // One message, always the last (#150): whatever is being read, the
    // newest text is read after it, and a replay is that text alone.
    let mut r = Router::new();
    r.open("A", None, None, Some(Policy::Latest));
    push(&mut r, "A", &["old"]);
    assert_eq!(next(&mut r).unwrap(), "old"); // being read
    push(&mut r, "A", &["newer"]);
    push(&mut r, "A", &["newest"]);
    assert_eq!(drain(&mut r), ["newest"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(drain(&mut r), ["[ again manual]", "newest"]);
}

#[test]
fn queue_policy_keeps_every_entry_and_starts_a_new_batch_when_caught_up() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one", "two"]);
    assert_eq!(next(&mut r).unwrap(), "one");
    push(&mut r, "A", &["three"]); // joins the unread batch
    assert_eq!(drain(&mut r), ["two", "three"]);
    push(&mut r, "A", &["four"]); // caught up: a new batch
    assert_eq!(r.channel("A").unwrap().entries().len(), 1);
    assert_eq!(drain(&mut r), ["four"]);
}

#[test]
fn text_pushed_while_the_last_entry_is_read_joins_its_batch() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one"]);
    assert_eq!(next(&mut r).unwrap(), "one"); // caught up, but still being read
    push(&mut r, "A", &["two"]);
    assert_eq!(r.channel("A").unwrap().entries().len(), 2);
    assert_eq!(next(&mut r).unwrap(), "two");
    r.done(); // "two" was read to its end
    push(&mut r, "A", &["three"]);
    assert_eq!(r.channel("A").unwrap().entries().len(), 1);
}

#[test]
fn push_with_overrides_the_policy_and_can_go_first() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one", "two"]);
    r.push_with("A", "now", None, false, true).unwrap();
    assert_eq!(drain(&mut r), ["now", "one", "two"]);
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one", "two"]);
    r.push_with("A", "only", None, true, false).unwrap();
    assert_eq!(drain(&mut r), ["only"]);
}

#[test]
fn pushing_into_a_closed_channel_fails() {
    let mut r = router(&["A"]);
    assert!(r.close("A"));
    assert!(!r.close("A"));
    assert_eq!(r.push("A", "x", None), None);
    assert!(!r.focus("A"));
}

#[test]
fn reopening_keeps_the_entries_and_updates_the_label() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["one"]);
    assert!(!r.open("A", Some("renamed".into()), Some("tab-2".into()), None));
    let c = r.channel("A").unwrap();
    assert_eq!(c.label.as_deref(), Some("renamed"));
    assert_eq!(c.host_tab.as_deref(), Some("tab-2"));
    assert_eq!(c.policy, Policy::Queue);
    assert_eq!(c.pending(), 1);
}

#[test]
fn closing_the_reading_channel_hands_off_with_an_announcement() {
    // #241: the user heard A last, so B's text is announced.
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.close("A");
    assert_eq!(r.active(), None);
    assert_eq!(r.last_active(), None);
    assert_eq!(drain(&mut r), ["[b]", "b1"]);
}

#[test]
fn flush_skips_to_the_end_but_keeps_entries_replayable() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(r.flush(Some("B")), 1);
    assert_eq!(drain(&mut r), ["a1", "a2"]);
    push(&mut r, "B", &["b2"]);
    push(&mut r, "A", &["a3"]);
    assert_eq!(r.flush(None), 2);
    assert_eq!(drain(&mut r), Vec::<String>::new());
    assert!(r.replay("A"));
    assert_eq!(drain(&mut r), ["a3"]);
}

#[test]
fn take_floor_announces_a_switch_from_another_channel() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.push_with("B", "urgent", None, false, true).unwrap();
    assert!(r.take_floor("B"));
    assert_eq!(drain(&mut r), ["[b]", "urgent", "[a]", "a2"]);
}

#[test]
fn replay_of_the_engaged_channel_is_not_announced() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(drain(&mut r), ["a1", "a2"]);
    assert_eq!(r.engaged(), Some("A"));
    assert!(r.replay("A"));
    assert_eq!(drain(&mut r), ["a1", "a2"]);
    assert!(!r.replay("B"), "nothing to replay");
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["[b]", "b1"]);
    assert!(r.replay("A"));
    assert_eq!(drain(&mut r), ["[a again manual]", "a1", "a2"]);
}

#[test]
fn pending_and_has_work() {
    let mut r = router(&["A", "B"]);
    assert!(!r.has_work());
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(r.pending(), 3);
    assert!(r.has_work());
    drain(&mut r);
    assert_eq!(r.pending(), 0);
    assert!(!r.has_work());
}

#[test]
fn a_prioritized_channel_preempts_the_batch_reading_now() {
    // Python test_background_decision_preempts_current_reader: a decision
    // in another channel is read right after the current entry, before the
    // rest of the reading channel's batch, which then resumes.
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["question"]);
    assert!(r.prioritize("B"));
    assert_eq!(r.prioritized(), ["B"]);
    assert_eq!(drain(&mut r), ["[b]", "question", "[a]", "a2"]);
    assert!(
        r.prioritized().is_empty(),
        "drained channels leave the list"
    );
}

#[test]
fn prioritized_channels_are_read_oldest_first_and_closing_forgets_them() {
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "C", &["c1"]);
    push(&mut r, "B", &["b1"]);
    r.prioritize("C");
    r.prioritize("B");
    r.prioritize("C");
    assert_eq!(r.prioritized(), ["C", "B"]);
    assert!(!r.prioritize("Z"), "not open");
    r.close("C");
    assert_eq!(r.prioritized(), ["B"]);
    assert_eq!(drain(&mut r), ["b1"]);
}

// -- per-channel mute (#196) and the focus-only gate (#195) ---------------

#[test]
fn a_muted_channel_waits_and_is_read_once_unmuted() {
    let mut r = router(&["A", "B"]);
    r.set_muted("A", true);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["b1"]);
    assert_eq!(r.channel("A").unwrap().pending(), 1, "kept, unread");
    r.set_muted("A", false);
    assert_eq!(drain(&mut r), ["[a]", "a1"]);
}

#[test]
fn a_muted_channel_loses_the_floor_mid_batch() {
    let mut r = router(&["A"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.set_muted("A", true);
    assert_eq!(next(&mut r), None);
}

#[test]
fn mute_is_kept_by_id_for_a_channel_opened_later() {
    let mut r = Router::new();
    r.set_muted("late", true);
    assert!(r.is_muted("late"));
    r.open("late", None, None, Some(Policy::Queue));
    assert!(r.channel("late").unwrap().muted());
    push(&mut r, "late", &["x"]);
    assert_eq!(next(&mut r), None);
}

#[test]
fn a_muted_decision_does_not_preempt() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["question"]);
    r.set_muted("B", true);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["a2"]);
}

#[test]
fn next_channel_skips_a_muted_channel_unless_all_are_muted() {
    // router.py: a muted session never takes the floor on a manual cycle;
    // with every session muted the plain ring is used (never a dead end).
    let mut r = router(&["A", "B", "C"]);
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    push(&mut r, "C", &["c1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.set_muted("B", true);
    assert_eq!(r.next_channel().unwrap().0, "C");
    r.set_muted("A", true);
    r.set_muted("C", true);
    assert_eq!(r.next_channel().unwrap().0, "A", "plain ring");
}

#[test]
fn focus_only_reads_the_focused_channel_and_holds_the_others() {
    // sessions.py earcon_only: only the foreground session gets voice time.
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    r.focus("A");
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r), None, "B waits");
    push(&mut r, "A", &["a1"]);
    assert_eq!(drain(&mut r), ["a1"]);
    assert_eq!(r.channel("B").unwrap().pending(), 1);
    // Focusing B (its prompt) reads it.
    r.focus("B");
    assert_eq!(drain(&mut r), ["[b]", "b1"]);
}

#[test]
fn focus_only_holds_a_background_decision_too() {
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    r.focus("A");
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(next(&mut r), None);
}

#[test]
fn focus_only_without_a_focus_holds_nothing_back() {
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["b1"]);
}

#[test]
fn an_authorized_channel_is_read_until_it_drains() {
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    r.focus("A");
    push(&mut r, "B", &["summary"]);
    assert!(r.authorize("B"));
    assert_eq!(drain(&mut r), ["summary"]);
    assert!(!r.is_authorized("B"), "spent once drained");
    push(&mut r, "B", &["later"]);
    assert_eq!(next(&mut r), None, "held again");
    assert!(!r.authorize("zz"));
}

#[test]
fn the_previous_focus_finishes_what_it_had_when_the_focus_moves() {
    // ingest.py cooperative hand-off: the old foreground drains first.
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    r.focus("A");
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.focus("B");
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a2", "[b]", "b1"]);
}

#[test]
fn a_replay_and_next_channel_read_a_background_channel() {
    let mut r = router(&["A", "B"]);
    r.set_focus_only(true);
    r.focus("A");
    push(&mut r, "B", &["b1"]);
    assert_eq!(r.next_channel().unwrap().0, "B");
    assert_eq!(drain(&mut r), ["[b manual]", "b1"]);
    assert!(r.replay("B"));
    assert_eq!(drain(&mut r), ["b1"]);
}

// -- agent batches (#243) ------------------------------------------------------

fn texts(r: &Router, id: &str) -> Vec<String> {
    r.channel(id)
        .unwrap()
        .entries()
        .iter()
        .map(|e| e.text.clone())
        .collect()
}

#[test]
fn an_agent_batch_grows_until_the_channel_is_flushed() {
    let mut r = router(&["A"]);
    r.append("A", "one", false, false).unwrap();
    assert_eq!(drain(&mut r), ["one"]);
    r.done();
    // Caught up: L3's next text joins the batch, it does not replace it.
    r.append("A", "two", false, false).unwrap();
    assert_eq!(drain(&mut r), ["two"]);
    assert_eq!(texts(&r, "A"), ["one", "two"]);
    // A flush of the channel (a new turn) starts a new batch.
    r.flush(Some("A"));
    r.append("A", "three", false, false).unwrap();
    assert_eq!(texts(&r, "A"), ["three"]);
}

#[test]
fn stored_text_is_never_fed_and_a_manual_return_reads_it() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["b1"], "the first reader is not announced");
    r.done();
    r.append("A", "stored one", false, true).unwrap();
    r.append("A", "stored two", false, true).unwrap();
    assert_eq!(drain(&mut r), Vec::<String>::new(), "never fed");
    assert_eq!(cursor(&r, "A"), 2);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(
        drain(&mut r),
        ["[a again manual]", "stored one", "stored two"]
    );
}

#[test]
fn stored_text_during_a_replay_is_read_with_it() {
    let mut r = router(&["A"]);
    r.append("A", "one", false, false).unwrap();
    assert_eq!(drain(&mut r), ["one"]);
    r.done();
    assert!(r.replay("A"));
    r.append("A", "two", false, true).unwrap();
    assert_eq!(drain(&mut r), ["one", "two"]);
}

#[test]
fn drop_decisions_takes_answered_decisions_out_of_the_batch() {
    let mut r = router(&["A"]);
    r.append("A", "lead in", false, false).unwrap();
    r.append("A", "question", true, false).unwrap();
    assert_eq!(next(&mut r).unwrap(), "lead in");
    r.append("A", "after", false, false).unwrap();
    // The question is unread: it is returned (reported dropped).
    let unread: Vec<String> = r
        .drop_decisions("A", Resolved::All)
        .into_iter()
        .map(|e| e.text)
        .collect();
    assert_eq!(unread, ["question"]);
    assert_eq!(texts(&r, "A"), ["lead in", "after"]);
    assert_eq!(cursor(&r, "A"), 1, "the cursor stays on the next entry");
    assert_eq!(drain(&mut r), ["after"]);
    r.done();
    assert!(r.replay("A"));
    assert_eq!(drain(&mut r), ["lead in", "after"]);
    assert!(r.drop_decisions("A", Resolved::All).is_empty());
}

#[test]
fn restart_target_before_any_read_is_the_channel_written_last() {
    let mut r = router(&["A", "B"]);
    assert_eq!(r.written_last(), None);
    r.append("B", "b", false, true).unwrap();
    r.append("A", "a", false, true).unwrap();
    assert_eq!(r.written_last().as_deref(), Some("A"));
}

#[test]
fn stored_text_while_a_replay_reads_its_last_entry_is_read_with_it() {
    let mut r = router(&["A"]);
    r.append("A", "one", false, false).unwrap();
    assert_eq!(drain(&mut r), ["one"]);
    r.done();
    assert!(r.replay("A"));
    // The replay is reading its last entry (nothing unread is left).
    assert_eq!(next(&mut r).unwrap(), "one");
    r.append("A", "two", false, true).unwrap();
    assert_eq!(drain(&mut r), ["two"]);
}

#[test]
fn a_tool_takes_out_only_the_decisions_read_aloud() {
    let mut r = router(&["A"]);
    r.append("A", "heard permission", true, false).unwrap();
    assert_eq!(next(&mut r).unwrap(), "heard permission");
    r.done();
    r.append("A", "stored question", true, true).unwrap();
    r.append("A", "unread permission", true, false).unwrap();
    let unread = r.drop_decisions("A", Resolved::Heard);
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(texts(&r, "A"), ["stored question", "unread permission"]);
    assert_eq!(drain(&mut r), ["unread permission"]);
    r.done();
    // The turn ended: what was read or stored is settled.
    r.append("A", "late unread permission", true, false)
        .unwrap();
    assert!(r.drop_decisions("A", Resolved::Settled).is_empty());
    assert_eq!(texts(&r, "A"), ["late unread permission"]);
}

#[test]
fn ending_a_batch_makes_the_next_agent_text_start_a_new_one() {
    let mut r = router(&["A"]);
    assert!(!r.is_agent("A"));
    r.append("A", "old turn", false, false).unwrap();
    assert!(r.is_agent("A"));
    assert_eq!(drain(&mut r), ["old turn"]);
    r.done();
    r.end_batch("A");
    r.append("A", "new turn", false, false).unwrap();
    assert_eq!(texts(&r, "A"), ["new turn"]);
}

// -- a replay holds the floor (#271) ---------------------------------------

/// `A` read `a1..a3` to the end, then the user restarts it (Up while idle):
/// the replay has read `a1`.
fn replaying_a(ids: &[&str]) -> Router {
    let mut r = router(ids);
    push(&mut r, "A", &["a1", "a2", "a3"]);
    drain(&mut r);
    assert!(r.replay("A"));
    assert_eq!(next(&mut r).unwrap(), "a1");
    r
}

#[test]
fn a_replay_is_not_taken_over_by_another_sessions_question() {
    let mut r = replaying_a(&["A", "B"]);
    push(&mut r, "B", &["question"]);
    assert!(r.prioritize("B"));
    assert_eq!(drain(&mut r), ["a2", "a3", "[b]", "question"]);
    assert!(r.prioritized().is_empty());
}

#[test]
fn a_replay_by_next_channel_is_not_taken_over_by_another_sessions_question() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "a2", "[b]", "b1"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), true)));
    assert_eq!(next(&mut r).unwrap(), "[a again manual]");
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["a2", "[b]", "question"]);
}

#[test]
fn a_replay_is_not_taken_over_by_another_sessions_message() {
    // B opened first and is focused (the user last prompted there): its
    // new message still waits for the replay's batch to end.
    let mut r = replaying_a(&["B", "A"]);
    r.focus("B");
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a2", "a3", "[b]", "b1"]);
}

#[test]
fn what_arrived_during_a_replay_is_read_after_it_decisions_first() {
    let mut r = replaying_a(&["C", "A", "B"]);
    push(&mut r, "C", &["c1"]);
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["a2", "a3", "[b]", "question", "[c]", "c1"]);
}

#[test]
fn flush_during_a_replay_moves_on() {
    // The flush hotkey (and a turn_start in the replayed session, which
    // flushes it) ends the hold: the question is read next.
    let mut r = replaying_a(&["A", "B"]);
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    r.flush(Some("A"));
    assert_eq!(drain(&mut r), ["[b]", "question"]);
}

#[test]
fn next_channel_and_mute_during_a_replay_move_on() {
    let mut r = replaying_a(&["A", "B", "C"]);
    push(&mut r, "B", &["b1"]);
    push(&mut r, "C", &["question"]);
    r.prioritize("C");
    // next_channel lands on B's unread message: not a replay, no hold
    // (live reading: the question goes first, then B resumes).
    assert_eq!(r.next_channel(), Some(("B".to_string(), false)));
    assert_eq!(
        drain(&mut r),
        ["[b manual]", "[c]", "question", "[b]", "b1"]
    );

    let mut r = replaying_a(&["A", "B"]);
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    r.set_muted("A", true);
    assert_eq!(drain(&mut r), ["[b]", "question"]);
}

#[test]
fn a_restart_with_a_channel_mid_read_holds_the_floor_too() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    drain(&mut r);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "[b]");
    assert!(r.replay("A"));
    assert_eq!(next(&mut r).unwrap(), "[a again manual]");
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["a2", "[b]", "b1", "question"]);
}

#[test]
fn reading_returns_to_a_batch_cut_by_a_priority() {
    // A reads live; C (opened first) has a waiting message; B's question
    // cuts A's batch. A resumes right after the question, before C.
    let mut r = router(&["C", "A", "B"]);
    r.focus("A");
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.focus("C");
    push(&mut r, "C", &["c1"]);
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["[b]", "question", "[a]", "a2", "[c]", "c1"]);
}

#[test]
fn reading_returns_to_a_batch_cut_by_a_priority_under_focus_only() {
    // No stall: A reads by next_channel (not focused, not authorized) when
    // the focused session's question cuts in; A's rest is still read.
    let mut r = router(&["F", "A"]);
    r.set_focus_only(true);
    r.focus("F");
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), false)));
    assert_eq!(next(&mut r).unwrap(), "[a manual]");
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "F", &["question"]);
    r.prioritize("F");
    assert_eq!(drain(&mut r), ["[f]", "question", "[a]", "a2"]);
}

#[test]
fn live_reading_still_lets_a_question_in() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["[b]", "question", "[a]", "a2"]);
}

#[test]
fn a_replay_does_not_hold_text_appended_after_it_started() {
    // Up replays a session that is still writing its turn: what it writes
    // during the replay is live reading, so a decision gets in after the
    // replayed entries.
    let mut r = router(&["A", "B"]);
    r.append("A", "a1", false, false).unwrap();
    r.append("A", "a2", false, false).unwrap();
    assert_eq!(drain(&mut r), ["a1", "a2"]);
    assert!(r.replay("A"));
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.append("A", "a3", false, false).unwrap();
    push(&mut r, "B", &["question"]);
    r.prioritize("B");
    assert_eq!(drain(&mut r), ["a2", "[b]", "question", "[a]", "a3"]);
}

#[test]
fn a_channel_muted_mid_read_does_not_resume_past_the_focus_gate() {
    // A reads by next_channel while unfocused; the focused session's
    // question waits; the user mutes A. Unmuted later, A is gated again
    // like any unfocused channel: nothing was reading it any more.
    let mut r = router(&["F", "A"]);
    r.set_focus_only(true);
    r.focus("F");
    push(&mut r, "A", &["a1", "a2"]);
    assert_eq!(r.next_channel(), Some(("A".to_string(), false)));
    assert_eq!(next(&mut r).unwrap(), "[a manual]");
    assert_eq!(next(&mut r).unwrap(), "a1");
    push(&mut r, "F", &["question"]);
    r.prioritize("F");
    r.set_muted("A", true);
    assert_eq!(drain(&mut r), ["[f]", "question"]);
    r.set_muted("A", false);
    assert!(drain(&mut r).is_empty());
}
