//! Split one text into the chunks of one item.
use crate::assembler::{Chunk, ProseAssembler};

/// The spoken chunks of `text`: the M1 assembler and text rules run over the
/// whole text as one final delta, so one item = one text and its chunks are
/// its spoken sentences. Paragraph breaks do not create chunks. Empty when
/// nothing in the text is speakable.
pub fn split_chunks(text: &str) -> Vec<String> {
    ProseAssembler::new()
        .feed(text, 0, true)
        .into_iter()
        .filter_map(|c| match c {
            Chunk::Text(s) | Chunk::Code(s) if !s.trim().is_empty() => Some(s),
            _ => None,
        })
        .collect()
}

/// The least a joined chunk after the first may grow to (`join_chunks`).
pub const JOIN_MIN: usize = 200;

/// Join sentence chunks for an engine that asks for longer ones
/// (`Engine::chunk_chars`, #235): the first stays alone, so reading starts
/// after one sentence; each later chunk joins whole sentences, with a
/// space, up to twice the length of the chunk before it (at least
/// `JOIN_MIN`) and never past `max` characters, so the next chunk is ready
/// before the playing one ends. A sentence longer than that stays whole.
/// `max` 0 leaves the chunks as they are.
pub fn join_chunks(chunks: Vec<String>, max: usize) -> Vec<String> {
    if max == 0 || chunks.len() < 2 {
        return chunks;
    }
    let len = |s: &str| s.chars().count();
    let budget = |prev: usize| (2 * prev).max(JOIN_MIN).min(max);
    let mut it = chunks.into_iter();
    let first = it.next().unwrap_or_default();
    let mut limit = budget(len(&first));
    let mut out = vec![first];
    let mut cur = String::new();
    for c in it {
        if cur.is_empty() {
            cur = c;
        } else if len(&cur) + 1 + len(&c) <= limit {
            cur.push(' ');
            cur.push_str(&c);
        } else {
            limit = budget(len(&cur));
            out.push(std::mem::replace(&mut cur, c));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Join sentence chunks up to `max` characters from the first one on, for
/// an engine that trades the fast start for fewer requests (no quick start,
/// review of #235): a text under `max` is one chunk. A sentence longer
/// than `max` stays whole. `max` 0 leaves the chunks as they are.
pub fn join_whole(chunks: Vec<String>, max: usize) -> Vec<String> {
    if max == 0 {
        return chunks;
    }
    let len = |s: &str| s.chars().count();
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in chunks {
        if cur.is_empty() {
            cur = c;
        } else if len(&cur) + 1 + len(&c) <= max {
            cur.push(' ');
            cur.push_str(&c);
        } else {
            out.push(std::mem::replace(&mut cur, c));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
