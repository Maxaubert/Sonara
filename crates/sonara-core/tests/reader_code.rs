//! Reader: code chunks (#295). A marked code line is a chunk of its own,
//! and with step mode on the reader stops after a block's announcement and
//! after each line until `NextLine`, `PreviousLine` or Play.
mod common;

use common::Host;
use sonara_core::code_speech::{CODE_LINE, CODE_START};
use sonara_core::reader::{Control, ItemId, QueueMode};

/// Prose, a block of three lines, its end and more prose.
fn block() -> String {
    format!(
        "Here it is.\n{CODE_START}Python code block, 3 lines.\n{CODE_LINE}Line 1. a.\n\
         {CODE_LINE}Line 2. b.\n{CODE_LINE}Line 3. c.\nEnd of Python code. Done."
    )
}

fn chunk_of(h: &Host) -> usize {
    h.playing().expect("a chunk is loaded").1
}

#[test]
fn each_code_line_is_its_own_chunk_and_shown_without_its_marker() {
    let mut h = Host::new();
    h.speak(&block());
    let item = h.reader.current().expect("reading").clone();
    assert_eq!(item.chunks.len(), 7, "{:?}", item.chunks);
    assert_eq!(item.chunks[2], format!("{CODE_LINE}Line 1. a."));
    h.finish_chunk();
    let np = h.state().now_playing.expect("playing");
    assert_eq!(np.text, "Python code block, 3 lines.");
    assert!(np.code);
    assert!(h.reader.in_code());
}

#[test]
fn without_step_mode_a_block_reads_straight_through() {
    let mut h = Host::new();
    h.speak(&block());
    h.play_out();
    assert_eq!(h.heard.len(), 7);
}

#[test]
fn step_mode_waits_after_the_announcement_and_after_each_line() {
    let mut h = Host::new();
    let fx = h.reader.set_code_step(true);
    assert!(fx.is_empty() || fx.len() == 1);
    let id = h.speak(&block());
    h.play_out(); // "Here it is.", the announcement, then it waits.
    assert_eq!(h.heard, [(id, 0), (id, 1)]);
    assert!(h.state().paused, "waits at line 1");
    assert_eq!(h.state().now_playing.unwrap().chunk, 2);
    // Next: line 1, then it waits at line 2.
    h.ctl(Control::NextLine);
    assert_eq!(chunk_of(&h), 2);
    h.finish_chunk();
    assert!(h.state().paused);
    assert_eq!(h.state().now_playing.unwrap().chunk, 3);
    // Previous: line 1 again.
    h.ctl(Control::PreviousLine);
    assert_eq!(chunk_of(&h), 2);
    h.finish_chunk();
    assert_eq!(h.state().now_playing.unwrap().chunk, 3);
    // Play: the rest straight through, then the prose.
    h.ctl(Control::Toggle);
    h.play_out();
    let heard: Vec<usize> = h.heard.iter().map(|(_, c)| *c).collect();
    assert_eq!(heard, [0, 1, 2, 2, 3, 4, 5, 6]);
    assert!(!h.state().paused);
}

#[test]
fn step_mode_waits_after_the_last_line_until_the_user_moves_on() {
    // 2026-10-14: the prose after a block no longer reads on its own, so
    // the last line can be heard again.
    let mut h = Host::new();
    h.reader.set_code_step(true);
    let id: ItemId = h.speak(&block());
    h.play_out();
    for _ in 0..3 {
        h.ctl(Control::NextLine);
        h.finish_chunk();
    }
    let s = h.state();
    assert!(s.paused && s.stepping, "waits after line 3");
    assert_eq!(s.now_playing.as_ref().unwrap().chunk, 5);
    assert!(s.now_playing.unwrap().code, "the line keys still work");
    // Previous: line 3 again, then the same wait.
    h.ctl(Control::PreviousLine);
    assert_eq!(chunk_of(&h), 4);
    h.finish_chunk();
    assert!(h.state().paused && h.state().stepping);
    // Next: the end and the prose read on.
    h.ctl(Control::NextLine);
    assert_eq!(chunk_of(&h), 5);
    h.play_out();
    assert_eq!(h.heard.last(), Some(&(id, 6)));
    assert!(!h.state().paused);
}

#[test]
fn play_during_a_block_reads_on_into_the_prose() {
    let (mut h, id) = waiting();
    h.ctl(Control::Toggle);
    h.play_out();
    assert_eq!(h.heard.last(), Some(&(id, 6)));
    assert!(!h.state().paused);
}

#[test]
fn next_line_while_a_line_plays_moves_on_and_previous_restarts_the_first() {
    let mut h = Host::new();
    h.speak(&block());
    h.finish_chunk();
    h.finish_chunk(); // now line 1 plays (no step mode)
    assert_eq!(chunk_of(&h), 2);
    h.ctl(Control::NextLine);
    assert_eq!(chunk_of(&h), 3);
    h.ctl(Control::PreviousLine);
    assert_eq!(chunk_of(&h), 2);
    h.ctl(Control::PreviousLine);
    assert_eq!(chunk_of(&h), 2, "line 1 has no line before it: it restarts");
}

#[test]
fn line_controls_do_nothing_outside_code() {
    let mut h = Host::new();
    h.speak("One. Two.");
    assert!(!h.reader.in_code());
    h.ctl(Control::NextLine);
    assert!(h.last.is_empty(), "{:?}", h.last);
    h.ctl(Control::PreviousLine);
    assert!(h.last.is_empty(), "{:?}", h.last);
}

/// Step mode on, waiting at line 1 of `block()`.
fn waiting() -> (Host, ItemId) {
    let mut h = Host::new();
    h.reader.set_code_step(true);
    let id = h.speak(&block());
    h.play_out();
    assert!(h.state().paused);
    assert!(h.state().stepping, "a step wait, not a user pause");
    (h, id)
}

#[test]
fn restart_during_a_step_wait_plays_from_the_top() {
    let (mut h, id) = waiting();
    h.ctl(Control::Restart);
    assert_eq!(h.plays_in_last(), [(id, 0)]);
    assert!(!h.state().paused);
}

#[test]
fn skip_during_a_step_wait_plays_the_next_item() {
    let (mut h, _) = waiting();
    let next = h.speak("Next message.");
    h.ctl(Control::Skip);
    assert_eq!(h.plays_in_last(), [(next, 0)]);
    assert!(!h.state().paused);
}

#[test]
fn an_interrupting_message_during_a_step_wait_plays() {
    let (mut h, _) = waiting();
    let next = h.speak_with("Urgent.", QueueMode::Append, true);
    assert_eq!(h.plays_in_last(), [(next, 0)]);
    assert!(!h.state().paused);
}

#[test]
fn a_user_pause_during_a_step_wait_still_holds() {
    let (mut h, _) = waiting();
    // Pause on a step wait makes it the user's pause: it holds.
    h.ctl(Control::Pause);
    assert!(!h.state().stepping);
    h.ctl(Control::Restart);
    assert!(h.plays_in_last().is_empty());
    assert!(h.state().paused);
}

#[test]
fn pause_and_play_mid_line_keeps_stepping() {
    let (mut h, _) = waiting();
    h.ctl(Control::NextLine); // line 1 plays
    h.ctl(Control::Pause);
    h.ctl(Control::Play);
    h.finish_chunk();
    assert!(h.state().paused, "still stepping: waits at line 2");
    assert_eq!(h.state().now_playing.unwrap().chunk, 3);
}

#[test]
fn a_user_pause_before_a_line_ends_stays_the_users_pause() {
    // Review #32: the line ends (or fails) after the user paused; the
    // pause must not turn into a step wait.
    let (mut h, _) = waiting();
    h.ctl(Control::NextLine); // line 1 plays
    h.ctl(Control::Pause);
    h.finish_chunk_racing_pause();
    let s = h.state();
    assert!(s.paused && !s.stepping, "the user's pause, not a step wait");
    h.ctl(Control::Restart);
    assert!(h.plays_in_last().is_empty(), "the pause holds");
    assert!(h.state().paused);
}

#[test]
fn a_user_pause_before_a_line_fails_stays_the_users_pause() {
    let (mut h, _) = waiting();
    h.ctl(Control::NextLine);
    h.ctl(Control::Pause);
    h.fail_chunk();
    let s = h.state();
    assert!(s.paused && !s.stepping, "the user's pause, not a step wait");
}

#[test]
fn turning_step_mode_off_during_a_wait_reads_on() {
    // Review #33: step mode applies at once, also during a wait.
    let (mut h, id) = waiting();
    h.code_step(false);
    let s = h.state();
    assert!(!s.paused && !s.stepping);
    assert_eq!(chunk_of(&h), 2, "line 1 plays");
    h.play_out();
    assert_eq!(h.heard.last(), Some(&(id, 6)));
}

#[test]
fn a_line_step_after_play_waits_again() {
    // Review #34: Play reads the block through, but a later next_line or
    // previous_line plays that line and waits again after it.
    let (mut h, _) = waiting();
    h.ctl(Control::NextLine);
    h.finish_chunk(); // waits at line 2
    h.ctl(Control::Toggle); // line 2 plays, the rest would read through
    assert_eq!(chunk_of(&h), 3);
    h.ctl(Control::PreviousLine);
    assert_eq!(chunk_of(&h), 2, "line 1 again");
    h.finish_chunk();
    let s = h.state();
    assert!(s.paused && s.stepping, "waits at line 2 again");
    assert_eq!(s.now_playing.unwrap().chunk, 3);
    // The same for next_line.
    h.ctl(Control::Toggle);
    assert_eq!(chunk_of(&h), 3);
    h.ctl(Control::NextLine);
    assert_eq!(chunk_of(&h), 4, "line 3");
    h.finish_chunk();
    assert!(h.state().paused && h.state().stepping, "waits after line 3");
}

#[test]
fn restart_into_a_code_block_steps_again() {
    let mut h = Host::new();
    h.reader.set_code_step(true);
    let text = format!(
        "{CODE_START}Python code block, 2 lines.\n{CODE_LINE}Line 1. a.\n{CODE_LINE}Line 2. b."
    );
    h.speak(&text);
    h.play_out(); // the announcement, then the wait at line 1
    h.ctl(Control::Play); // the rest straight through
    h.ctl(Control::Restart);
    h.finish_chunk(); // the announcement again
    assert!(h.state().paused, "waits at line 1 again");
}

#[test]
fn text_in_its_spoken_form_is_cut_into_sentences_and_not_read_again() {
    // #294 review: the agent's prose was converted once; the reader must
    // not convert it a second time (inline code, the user's words).
    use sonara_core::code_speech::SPOKEN;
    use sonara_core::reader::{pack_message, split_chunks};
    use sonara_core::text::normalize_for_speech;
    for raw in [
        "The value `v1.2.3` is set.",
        "Run `HTTP_PORT=8080` first.",
        "Released v1.20 on 12.12.2026 at 14:30.",
    ] {
        let once = normalize_for_speech(raw);
        let spoken = format!("{SPOKEN}{once}");
        assert_eq!(split_chunks(&spoken), std::slice::from_ref(&once), "{raw}");
        assert_eq!(
            pack_message(&spoken, 700, false),
            std::slice::from_ref(&once),
            "{raw}"
        );
    }
    let two = format!("{SPOKEN}One point five. Done!\n\nNext one.");
    assert_eq!(
        split_chunks(&two),
        ["One point five.", "Done!", "Next one."]
    );
    assert_eq!(
        pack_message(&two, 700, false),
        ["One point five. Done!\n\nNext one."]
    );
}
