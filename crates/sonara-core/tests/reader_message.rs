//! Reader: send mode `message` (#235, `set_chunking`). An engine billed or
//! limited per request reads an item as one chunk: its paragraphs joined
//! with a blank line, its sentences with a space, split only past the
//! engine's input limit (at paragraph, then sentence boundaries, as few
//! chunks as possible). `Sentences` (the default) keeps one sentence per
//! chunk.
mod common;

use common::Host;
use sonara_core::reader::{pack_message, split_chunks, Chunking, Control};

fn chunks_of(h: &Host) -> Vec<String> {
    h.reader.current().unwrap().chunks.clone()
}

/// `n` sentences of exactly `len` characters ("Aaaa." with the period).
fn sentences(n: usize, len: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let c = (b'A' + (i % 26) as u8) as char;
            format!("{c}{}.", "a".repeat(len - 2))
        })
        .collect()
}

fn message(limit: usize) -> Chunking {
    Chunking::Message {
        max: limit,
        bytes: false,
    }
}

#[test]
fn sentences_by_default_one_chunk_each() {
    let mut h = Host::new();
    assert_eq!(h.reader.chunking(), Chunking::Sentences);
    h.speak("One. Two. Three.");
    assert_eq!(chunks_of(&h), vec!["One.", "Two.", "Three."]);
}

/// The evidence of 2026-10-04: a reply of 18 sentences was 18 requests.
/// In message mode it is one chunk, its paragraphs kept.
#[test]
fn a_whole_reply_is_one_chunk_with_its_paragraphs() {
    let mut h = Host::new();
    let _ = h.reader.set_chunking(message(4000));
    let s = sentences(18, 40);
    let text = format!(
        "{}\n\n{}\n\n{}",
        s[..6].join(" "),
        s[6..12].join(" "),
        s[12..].join(" ")
    );
    h.speak(&text);
    let chunks = chunks_of(&h);
    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(chunks[0], text, "paragraph breaks kept, nothing lost");
    let st = h.reader.state();
    assert_eq!(st.now_playing.unwrap().chunks, 1);
}

#[test]
fn past_the_limit_it_splits_at_paragraphs_first() {
    // Three paragraphs of 300 characters each, limit 700: two chunks, cut
    // at the paragraph boundary (a sentence cut would also give two).
    let p: Vec<String> = (0..3)
        .map(|i| {
            sentences(5, 60)[..]
                .join(" ")
                .replacen('A', &i.to_string(), 1)
        })
        .collect();
    let text = p.join("\n\n");
    let chunks = pack_message(&text, 700, false);
    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert_eq!(chunks[0], format!("{}\n\n{}", p[0], p[1]));
    assert_eq!(chunks[1], p[2]);
}

#[test]
fn as_few_chunks_as_possible_even_inside_a_paragraph() {
    // Paragraphs of 650 and 650 characters, limit 700: whole paragraphs
    // give two chunks, and so does any cut: paragraphs first.
    let a = sentences(10, 64).join(" ");
    let b = sentences(10, 64).join(" ");
    let chunks = pack_message(&format!("{a}\n\n{b}"), 700, false);
    assert_eq!(chunks, vec![a.clone(), b.clone()]);
    // One paragraph of 1300 characters, limit 700: split at sentences,
    // two chunks, each within the limit.
    let long = sentences(20, 64).join(" ");
    let chunks = pack_message(&long, 700, false);
    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert!(
        chunks.iter().all(|c| c.chars().count() <= 700),
        "{chunks:?}"
    );
    assert_eq!(chunks.join(" "), long);
    // Paragraphs of 400, 400 and 400, limit 1000: paragraph-first gives two
    // chunks (800 + 400), the fewest possible.
    let p = sentences(5, 79).join(" ");
    let chunks = pack_message(&format!("{p}\n\n{p}\n\n{p}"), 1000, false);
    assert_eq!(chunks.len(), 2, "{chunks:?}");
    // When whole paragraphs would need more chunks than a cut inside one,
    // the fewest chunks win: 600 + 600 + 600 at 900 is three by
    // paragraph, two by sentence.
    let q = sentences(10, 59).join(" ");
    let chunks = pack_message(&format!("{q}\n\n{q}\n\n{q}"), 900, false);
    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert!(
        chunks.iter().all(|c| c.chars().count() <= 900),
        "{chunks:?}"
    );
}

#[test]
fn a_sentence_longer_than_the_limit_stays_whole() {
    let long = sentences(1, 300).remove(0);
    let chunks = pack_message(&format!("Hi. {long} Bye."), 250, false);
    assert_eq!(chunks, vec!["Hi.".to_string(), long, "Bye.".into()]);
}

#[test]
fn bytes_count_for_a_byte_limit() {
    // "Grüße." is 6 characters and 8 bytes.
    let text = "Grüße. Grüße. Grüße.";
    assert_eq!(pack_message(text, 20, false).len(), 1);
    assert_eq!(pack_message(text, 20, true).len(), 2);
}

#[test]
fn the_text_rules_still_apply_and_nothing_speakable_is_skipped() {
    let text = "This is **bold**.\n\nAnd `x` too.";
    let chunks = pack_message(text, 4000, false);
    assert_eq!(chunks, vec![split_chunks(text).join("\n\n")]);
    assert!(!chunks[0].contains("**"));
    assert!(pack_message("  \n\n  ", 4000, false).is_empty());
    let mut h = Host::new();
    let _ = h.reader.set_chunking(message(4000));
    let id = h.speak("   ");
    assert!(
        h.reader.current().is_none(),
        "skipped, as in sentence mode: {id:?}"
    );
}

/// Up (Restart) replays the whole message from its one chunk, without
/// asking for it again (the host still has its audio).
#[test]
fn restart_replays_the_whole_message_without_a_new_synthesis() {
    let mut h = Host::new();
    let _ = h.reader.set_chunking(message(4000));
    h.speak("One. Two.\n\nThree.");
    let synth = h.synth_in_last();
    assert_eq!(synth.len(), 1);
    h.ctl(Control::Restart);
    assert!(
        h.synth_in_last().is_empty(),
        "the chunk was already asked for"
    );
    assert_eq!(h.reader.state().now_playing.unwrap().chunk, 0);
}

#[test]
fn items_queued_before_a_switch_keep_their_chunks() {
    let mut h = Host::new();
    h.speak("One. Two.");
    h.speak("Three. Four.");
    let _ = h.reader.set_chunking(message(4000));
    h.speak("Five. Six.");
    assert_eq!(chunks_of(&h), vec!["One.", "Two."]);
    let queued: Vec<Vec<String>> = h.reader.queued().map(|i| i.chunks.clone()).collect();
    assert_eq!(
        queued,
        vec![
            vec!["Three.".to_string(), "Four.".into()],
            vec!["Five. Six.".to_string()]
        ]
    );
}
