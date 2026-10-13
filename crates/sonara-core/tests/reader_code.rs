//! Reader: code chunks (#295). A marked code line is a chunk of its own,
//! and with step mode on the reader stops after a block's announcement and
//! after each line until `NextLine`, `PreviousLine` or Play.
mod common;

use common::Host;
use sonara_core::code_speech::{CODE_LINE, CODE_START};
use sonara_core::reader::{Control, ItemId};

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
fn step_mode_goes_on_to_the_prose_after_the_last_line() {
    let mut h = Host::new();
    h.reader.set_code_step(true);
    let id: ItemId = h.speak(&block());
    h.play_out();
    for _ in 0..3 {
        h.ctl(Control::NextLine);
        h.finish_chunk();
    }
    // After line 3 the end and the prose read on their own.
    assert!(!h.state().paused);
    h.play_out();
    assert_eq!(h.heard.last(), Some(&(id, 6)));
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
