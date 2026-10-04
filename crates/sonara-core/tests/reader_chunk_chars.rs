//! Reader: joined chunks (`set_chunk_chars`, #235): an engine that pays per
//! request (Gemini's free tier counts requests) asks for fewer, longer
//! chunks. The first chunk of an item stays one sentence, so reading starts
//! as fast as before; each later chunk joins sentences up to twice the
//! length of the one before it (at least `JOIN_MIN`), never more than the
//! engine's limit, so the next chunk is ready before the playing one ends.
mod common;

use common::Host;
use sonara_core::reader::{join_chunks, split_chunks, JOIN_MIN};

fn chunks_of(h: &Host) -> Vec<String> {
    h.reader.current().unwrap().chunks.clone()
}

fn sentences(n: usize, len: usize) -> String {
    // `n` sentences of exactly `len` characters ("Aaaa." with the period).
    (0..n)
        .map(|i| {
            let c = (b'A' + (i % 26) as u8) as char;
            format!("{c}{}.", "a".repeat(len - 2))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn zero_keeps_one_sentence_per_chunk() {
    let mut h = Host::new();
    let _ = h.reader.set_chunk_chars(0);
    h.speak("One. Two. Three.");
    assert_eq!(chunks_of(&h), vec!["One.", "Two.", "Three."]);
    assert_eq!(h.reader.chunk_chars(), 0);
}

#[test]
fn the_first_chunk_stays_one_sentence_and_later_ones_grow() {
    let text = sentences(30, 50);
    let plain = split_chunks(&text);
    assert_eq!(plain.len(), 30);
    let joined = join_chunks(plain.clone(), 1000);
    // 50, then up to 200 (JOIN_MIN), 400, 800, 1000, 1000...
    let lens: Vec<usize> = joined.iter().map(|c| c.chars().count()).collect();
    assert_eq!(lens[0], 50, "the first sentence alone");
    assert_eq!(lens[1], 50 * 3 + 2, "three sentences fit in 200");
    assert!(lens[2] <= 2 * lens[1] && lens[2] > lens[1], "{lens:?}");
    assert!(lens.iter().all(|&l| l <= 1000), "{lens:?}");
    assert!(joined.len() < 10, "far fewer requests: {lens:?}");
    // Nothing is lost or reordered: the sentences joined with spaces.
    assert_eq!(joined.join(" "), plain.join(" "));
}

#[test]
fn a_sentence_longer_than_the_limit_stays_whole() {
    let long = sentences(1, 300);
    let text = format!("Hi. {long} {long} Bye.");
    let joined = join_chunks(split_chunks(&text), 250);
    assert_eq!(joined.len(), 4, "{joined:?}");
    assert_eq!(joined[1], long);
    assert_eq!(joined[2], long);
    assert_eq!(joined[3], "Bye.");
}

#[test]
fn small_limits_and_short_items() {
    assert_eq!(join_chunks(vec!["One.".into()], 1000), vec!["One."]);
    assert_eq!(
        join_chunks(vec!["A.".into(), "B.".into(), "C.".into()], 1000),
        vec!["A.", "B. C."]
    );
    // A limit under JOIN_MIN is still the limit.
    let joined = join_chunks(vec!["A.".into(), "Bb.".into(), "Cc.".into()], 5);
    assert_eq!(joined, vec!["A.", "Bb.", "Cc."]);
    assert_eq!(JOIN_MIN, 200);
}

#[test]
fn the_setting_applies_to_items_spoken_after_it() {
    let mut h = Host::new();
    h.speak("One. Two. Three.");
    let _ = h.reader.set_chunk_chars(1000);
    assert_eq!(chunks_of(&h).len(), 3, "the current item keeps its chunks");
    h.ctl(sonara_core::reader::Control::Stop);
    h.speak("One. Two. Three.");
    assert_eq!(chunks_of(&h), vec!["One.", "Two. Three."]);
}
