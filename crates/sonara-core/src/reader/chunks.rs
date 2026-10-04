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

/// The spoken paragraphs of `text`, each a list of its sentences (the
/// assembler and text rules as `split_chunks`).
fn paragraphs(text: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for c in ProseAssembler::new().feed(text, 0, true) {
        match c {
            Chunk::Text(s) | Chunk::Code(s) if !s.trim().is_empty() => cur.push(s),
            Chunk::ParagraphBreak if !cur.is_empty() => out.push(std::mem::take(&mut cur)),
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The chunks of `text` in send mode `message` (#235): one chunk, its
/// paragraphs joined with a blank line and its sentences with a space,
/// unless it is longer than `max` (characters, or UTF-8 bytes with
/// `bytes`). Then it is cut at paragraph boundaries, and inside a
/// paragraph that does not fit at sentence boundaries, into as few chunks
/// as possible: when cutting only between paragraphs would need more
/// chunks than cutting anywhere between sentences, the fewest chunks win.
/// A sentence longer than `max` stays whole (the engine splits it at
/// words). Empty when nothing in the text is speakable.
pub fn pack_message(text: &str, max: usize, bytes: bool) -> Vec<String> {
    let size = |s: &str| if bytes { s.len() } else { s.chars().count() };
    let max = max.max(1);
    let paras = paragraphs(text);
    // Every sentence, with whether it starts a paragraph.
    let units: Vec<(bool, &str)> = paras
        .iter()
        .flat_map(|p| p.iter().enumerate().map(|(i, s)| (i == 0, s.as_str())))
        .collect();
    let join = |cur: &str, start: bool, s: &str| {
        if cur.is_empty() {
            s.to_string()
        } else if start {
            format!("{cur}\n\n{s}")
        } else {
            format!("{cur} {s}")
        }
    };
    // Fewest chunks: fill each up to the limit, sentence by sentence.
    let pack = |units: &[(bool, &str)]| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for &(start, s) in units {
            let next = join(&cur, start, s);
            if !cur.is_empty() && size(&next) > max {
                out.push(std::mem::replace(&mut cur, s.to_string()));
            } else {
                cur = next;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let fewest = pack(&units);
    if fewest.len() <= 1 {
        return fewest;
    }
    // Paragraphs first: a chunk ends only between paragraphs, except
    // inside a paragraph that does not fit in a chunk of its own.
    let mut by_para: Vec<String> = Vec::new();
    let mut cur = String::new();
    for p in &paras {
        let whole = p.join(" ");
        let next = join(&cur, true, &whole);
        if size(&next) <= max {
            cur = next;
            continue;
        }
        if !cur.is_empty() {
            by_para.push(std::mem::take(&mut cur));
        }
        if size(&whole) <= max {
            cur = whole;
        } else {
            let us: Vec<(bool, &str)> = p.iter().map(|s| (false, s.as_str())).collect();
            let mut parts = pack(&us);
            cur = parts.pop().unwrap_or_default();
            by_para.extend(parts);
        }
    }
    if !cur.is_empty() {
        by_para.push(cur);
    }
    if by_para.len() <= fewest.len() {
        by_para
    } else {
        fewest
    }
}
