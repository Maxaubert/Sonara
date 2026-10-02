//! The pure router, ported from the Python plugin's `tests/test_router.py`
//! and `tests/test_channel.py` (the parts that are not agent features), plus
//! the L2 policies. Issue numbers name the Python regressions they guard.
use sonara_channels::{Feed, Policy, Router};

/// One feed in compact notation: an entry's text, or `[label]` for an
/// announcement (`again` for a replay, `manual` for `next_channel`).
fn show(f: Option<Feed>) -> Option<String> {
    f.map(|f| match f {
        Feed::Entry { entry, .. } => entry.text,
        Feed::Announce {
            label,
            replay,
            manual,
            ..
        } => format!(
            "[{label}{}{}]",
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
fn a_channel_without_a_label_is_not_announced() {
    let mut r = Router::new();
    r.open("A", Some("alpha".into()), None, Some(Policy::Queue));
    r.open("B", None, None, Some(Policy::Queue));
    push(&mut r, "A", &["a1"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(drain(&mut r), ["a1", "b1"]);
    push(&mut r, "A", &["a2"]);
    assert_eq!(drain(&mut r), ["[alpha]", "a2"]);
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
    // A was the last reader and is gone: no announcement, no "again".
    assert_eq!(drain(&mut r), ["b2"]);
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
    assert_eq!(drain(&mut r), ["newest"]);
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
fn closing_the_reading_channel_hands_off_without_announcement() {
    let mut r = router(&["A", "B"]);
    push(&mut r, "A", &["a1", "a2"]);
    push(&mut r, "B", &["b1"]);
    assert_eq!(next(&mut r).unwrap(), "a1");
    r.close("A");
    assert_eq!(r.active(), None);
    assert_eq!(drain(&mut r), ["b1"]);
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
