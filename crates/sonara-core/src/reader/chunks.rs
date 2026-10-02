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
            Chunk::Text(s) if !s.trim().is_empty() => Some(s),
            _ => None,
        })
        .collect()
}
