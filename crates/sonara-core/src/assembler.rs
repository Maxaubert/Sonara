//! Assemble streamed text deltas into complete, speakable chunks.
//!
//! PURE: no I/O. Splits prose into sentences and turns each triple-backtick
//! fenced code block into its spoken announcement, its lines as read aloud
//! and its end (`code_speech`, #290, #295). Port of the retired
//! Python plugin's `assembler.py` (removed in #248), method for method; the
//! golden fixtures (`tests/fixtures/text_rules`) are the contract.
//!
//! Offsets (`emitted`, slice points into `buf` and `pending`) are BYTE offsets
//! into UTF-8 strings where Python uses code-point offsets. Every slice point
//! comes from `find`, a regex match end, or `len() - remainder.len()`, so each
//! one is a char boundary; `stabilize_ordinals` only swaps ASCII '.' for ':',
//! so it keeps byte lengths and boundaries exactly as Python keeps char counts.
use crate::code_speech;
use crate::table;
use crate::text::{normalize_for_speech, stabilize_ordinals};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

const FENCE: &str = "```";

// A complete sentence ends at . ! or ? (plus any closing quotes/brackets/
// markdown markers) followed by WHITESPACE. Requiring the whitespace keeps
// intra-token dots intact ("3.14", "daemon.py:123", "v2.1.3") and stops a
// delta boundary that happens to land right after a period from emitting a
// premature half-sentence; the trailing fragment is delivered by the final
// flush instead (#56). `[\s\x1C-\x1F]` is Python's `\s` (see text.rs).
static SENTENCE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)(.+?[.!?]["'’”)\]*_`]*)[\s\x1C-\x1F]+"#).unwrap());
// a chunk is speakable only if it contains at least one word character
static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z0-9]").unwrap());
// A paragraph boundary = a blank line. We split the RAW buffer on this (before
// cleaning collapses whitespace) so the boundary survives even when the
// blank line straddles two streamed deltas. feed() yields ParagraphBreak between
// paragraphs so the daemon can group history by paragraph (the nav 'item' unit).
static PARA: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n[ \t]*\n").unwrap());

/// One unit of `ProseAssembler::feed` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    /// A cleaned, speakable chunk (a sentence or a line).
    Text(String),
    /// A fenced code block, apart from the prose so a reader can skip it
    /// (#214): its announcement ("Python code block, 3 lines."), its
    /// spoken lines ("Line 1. ...", blank ones left out but counted) and
    /// its end ("End of Python code."), #295.
    Code {
        summary: String,
        lines: Vec<String>,
        end: String,
    },
    /// Emitted between paragraphs (Python's PARAGRAPH_BREAK sentinel).
    ParagraphBreak,
}

/// Python `str.isspace()`: Unicode White_Space plus U+001C..U+001F.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()` with no arguments.
fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `str.splitlines()`: splits on \n, \r\n, \r, \x0b, \x0c, \x1c, \x1d,
/// \x1e, \x85, U+2028 and U+2029; a trailing terminator adds no empty item.
fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\r' => {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                    i + 2
                } else {
                    i + 1
                }
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => i + c.len_utf8(),
            _ => continue,
        };
        out.push(&s[start..i]);
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[derive(Default)]
pub struct ProseAssembler {
    seen: HashSet<u32>,
    /// pending prose text (RAW, outside fences)
    buf: String,
    /// bytes of the CURRENT paragraph's RAW text already emitted
    emitted: usize,
    /// raw tail not yet split into a line/fence token
    pending: String,
    in_fence: bool,
    fence_lang: String,
    fence_lines: Vec<String>,
    /// have we consumed the opening info-string line?
    fence_opened_line: bool,
}

impl ProseAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, delta: &str, index: u32, is_final: bool) -> Vec<Chunk> {
        let mut out = Vec::new();
        if self.seen.contains(&index) {
            if index == 0 {
                // A NEW message block restarts at index 0. A lost final from the
                // previous block would otherwise leave seen poisoned and every
                // colliding delta of the new block silently DROPPED (deep audit
                // #25). Flush the stale block and start fresh; a true duplicate
                // first delta re-speaks at worst, never drops.
                out.extend(self.consume(true));
                out.extend(self.flush_prose());
                self.reset();
            } else {
                // still honor a final flush even on a duplicate index
                if is_final {
                    out.extend(self.flush_prose());
                    self.reset();
                }
                return out;
            }
        }
        self.seen.insert(index);

        self.pending.push_str(delta);
        out.extend(self.consume(false));

        if is_final {
            out.extend(self.consume(true));
            out.extend(self.flush_prose());
            self.reset();
        }
        out
    }

    /// Scan `pending` for fence boundaries, routing text to prose or fence.
    ///
    /// Only acts on text we can resolve: a fence marker, or (inside a fence)
    /// a complete line. Leftover ambiguous tail stays in `pending` unless force.
    fn consume(&mut self, force: bool) -> Vec<Chunk> {
        let mut out = Vec::new();
        loop {
            if self.in_fence {
                if let Some(nl) = self.pending.find('\n') {
                    let line = self.pending[..nl].to_string();
                    self.pending = self.pending[nl + 1..].to_string();
                    let stripped = py_strip(&line);
                    // A closing fence STARTS its line with >= 3 backticks and is
                    // followed by nothing (or whitespace + trailing prose, which
                    // resumes as prose). Matching ``` ANYWHERE closed early on
                    // code that CONTAINS a ``` literal, leaking code lines as
                    // spoken prose (deep audit #25). Backticks followed directly
                    // by a word ("```python") are content, not a close.
                    if let Some(close_rest) = self.closing_fence_rest(stripped) {
                        out.push(self.close_fence());
                        if !close_rest.is_empty() {
                            self.pending = format!("{close_rest}\n{}", self.pending);
                        }
                        continue;
                    }
                    if !self.fence_opened_line {
                        // first line after opening ``` is the info string
                        self.fence_lang = stripped.to_string();
                        self.fence_opened_line = true;
                    } else {
                        self.fence_lines.push(line);
                    }
                    continue;
                }
                // no complete line yet
                if force {
                    // unterminated fence at EOF: the tail is either a
                    // (newline-less) closing fence or a final content line
                    let tail = std::mem::take(&mut self.pending);
                    let stripped = py_strip(&tail);
                    if let Some(close_rest) = self.closing_fence_rest(stripped) {
                        out.push(self.close_fence());
                        if !close_rest.is_empty() {
                            self.pending = close_rest;
                            continue; // resume prose scan (still forced)
                        }
                    } else {
                        if !stripped.is_empty() {
                            if self.fence_opened_line {
                                self.fence_lines.push(tail.clone());
                            } else {
                                self.fence_lang = stripped.to_string();
                            }
                        }
                        out.push(self.close_fence());
                    }
                }
                break;
            }
            if let Some(open_at) = self.pending.find(FENCE) {
                let prose = self.pending[..open_at].to_string();
                self.buf.push_str(&prose);
                self.pending = self.pending[open_at + FENCE.len()..].to_string();
                out.extend(self.split_sentences());
                // An unterminated lead-in ("Here is the code:") must be
                // spoken BEFORE the fence summary -- it used to sit as the
                // buffered remainder until the final flush and play AFTER
                // the block it introduced (deep audit #25). The fence is a
                // hard boundary, so the lead-in is complete: flush it now.
                out.extend(self.flush_prose());
                self.in_fence = true;
                self.fence_opened_line = false;
                self.fence_lang.clear();
                self.fence_lines.clear();
                continue;
            }
            // no fence opening visible
            if force {
                let pending = std::mem::take(&mut self.pending);
                self.buf.push_str(&pending);
                out.extend(self.split_sentences());
            } else {
                // hold back only enough tail to detect a future "```";
                // commit everything except a possible partial fence marker
                let keep = self.partial_fence_tail_len();
                let commit = if keep > 0 {
                    // backticks are ASCII, so len - keep is a char boundary
                    let cut = self.pending.len() - keep;
                    let commit = self.pending[..cut].to_string();
                    self.pending = self.pending[cut..].to_string();
                    commit
                } else {
                    std::mem::take(&mut self.pending)
                };
                if !commit.is_empty() {
                    self.buf.push_str(&commit);
                    out.extend(self.split_sentences());
                }
            }
            break;
        }
        out
    }

    /// If `stripped` (a fence-interior line) is a closing fence, return the
    /// trailing prose after it ("" if none); else None. A close = the line
    /// STARTS with >= 3 backticks followed by end-of-line or whitespace; a
    /// word glued to the backticks ("```python") is content (deep audit #25).
    /// Only valid once the info-string line was consumed.
    fn closing_fence_rest(&self, stripped: &str) -> Option<String> {
        if !self.fence_opened_line || !stripped.starts_with(FENCE) {
            return None;
        }
        let ticks = stripped.len() - stripped.trim_start_matches('`').len();
        if ticks < 3 {
            return None;
        }
        let rest = &stripped[ticks..];
        if let Some(first) = rest.chars().next() {
            if first != ' ' && first != '\t' {
                return None;
            }
        }
        Some(py_strip(rest).to_string())
    }

    /// How many trailing bytes of `pending` could be the start of a fence.
    fn partial_fence_tail_len(&self) -> usize {
        for n in [2, 1] {
            if self.pending.ends_with(&"`".repeat(n)) {
                return n;
            }
        }
        0
    }

    fn close_fence(&mut self) -> Chunk {
        let n = self.fence_lines.len();
        let lang = std::mem::take(&mut self.fence_lang);
        let lines = std::mem::take(&mut self.fence_lines)
            .iter()
            .enumerate()
            .filter_map(|(i, l)| code_speech::line(l, &lang, i + 1))
            .collect();
        self.in_fence = false;
        self.fence_opened_line = false;
        Chunk::Code {
            summary: code_speech::announcement(&lang, n),
            lines,
            end: code_speech::end_of_code(&lang),
        }
    }

    /// Clean one RAW chunk and append it if speakable. Cleaning happens
    /// per-chunk AFTER splitting, so a markdown pair whose closing marker
    /// arrives in a later delta can never invalidate already-emitted text
    /// (the old cleaned-offset bookkeeping chopped characters, #56); an
    /// unpaired marker is simply stripped by normalize_for_speech.
    fn emit_chunk(raw: &str, out: &mut Vec<Chunk>) {
        let cleaned = normalize_for_speech(raw);
        if WORD.is_match(&cleaned) {
            out.push(Chunk::Text(cleaned));
        }
    }

    /// Emit `text`, each table as one chunk per row (#303) and the prose
    /// around it through `prose`.
    fn with_tables(text: &str, out: &mut Vec<Chunk>, prose: impl Fn(&str, &mut Vec<Chunk>)) {
        let mut rest = text;
        while let Some(t) = table::find(rest) {
            prose(&rest[..t.start], out);
            for row in &t.rows {
                Self::emit_chunk(row, out);
            }
            rest = &rest[t.end..];
        }
        prose(rest, out);
    }

    /// Split RAW `text` into cleaned sentences. Return (sentences, raw
    /// remainder). When keep_remainder is false the trailing fragment belongs
    /// to a complete paragraph: emit it too, one chunk per line, so a closing
    /// bullet list never leaves as one giant unpunctuated blob (#56). Tables
    /// are read row by row once their paragraph is complete; until then
    /// they wait in the remainder (#303).
    fn sentences_of(text: &str, keep_remainder: bool) -> (Vec<Chunk>, &str) {
        if keep_remainder {
            let hold = table::hold_from(text);
            let (out, remainder) = Self::prose_sentences(&text[..hold], true);
            return (out, &text[hold - remainder.len()..]);
        }
        let mut out = Vec::new();
        Self::with_tables(text, &mut out, |prose, out| {
            out.extend(Self::prose_sentences(prose, false).0)
        });
        (out, "")
    }

    fn prose_sentences(text: &str, keep_remainder: bool) -> (Vec<Chunk>, &str) {
        let mut out = Vec::new();
        let mut last_end = 0;
        for caps in SENTENCE.captures_iter(text) {
            Self::emit_chunk(&caps[1], &mut out);
            last_end = caps.get(0).map_or(last_end, |m| m.end());
        }
        let mut remainder = &text[last_end..];
        if !keep_remainder {
            for line in split_lines(remainder) {
                Self::emit_chunk(line, &mut out);
            }
            remainder = "";
        }
        (out, remainder)
    }

    /// Emit complete sentences from `buf`, with ParagraphBreak markers between
    /// paragraphs (blank-line boundaries). Keeps the trailing partial sentence.
    ///
    /// `buf` is kept RAW (uncleaned) and `emitted` counts RAW bytes of the
    /// current paragraph already emitted. Raw text is append-only under
    /// streaming, so the offset can never be invalidated by later deltas,
    /// unlike the previous cleaned-text offset, which desynced when a markdown
    /// pair straddled an already-emitted sentence (#56). The only pre-split
    /// rewrite is stabilize_ordinals, which is length-preserving so offsets
    /// stay valid.
    fn split_sentences(&mut self) -> Vec<Chunk> {
        let mut out = Vec::new();
        let buf = std::mem::take(&mut self.buf);
        let raw_paragraphs: Vec<&str> = PARA.split(&buf).collect();
        // regex split always yields at least one item, even for ""
        let (last_raw, complete) = raw_paragraphs
            .split_last()
            .expect("split yields at least one item");
        // All but the last are COMPLETE paragraphs (each was followed by a blank line).
        for raw_para in complete {
            let start = self.emitted.min(raw_para.len());
            let view = stabilize_ordinals(raw_para);
            let (sents, _) = Self::sentences_of(&view[start..], false);
            out.extend(sents);
            out.push(Chunk::ParagraphBreak);
            self.emitted = 0; // paragraph done; the next one starts fresh
        }
        // The last raw paragraph is the current, possibly-incomplete one.
        let start = self.emitted.min(last_raw.len());
        let view = stabilize_ordinals(last_raw);
        let (sents, remainder) = Self::sentences_of(&view[start..], true);
        out.extend(sents);
        self.emitted = last_raw.len() - remainder.len();
        self.buf = last_raw.to_string(); // keep RAW so a straddling blank line survives
        out
    }

    /// Emit the not-yet-emitted RAW tail, one chunk per line. The tail used
    /// to leave as ONE blob; a closing dash-bullet list (no terminal
    /// punctuation anywhere) then reached the engine as one 300+ char chunk
    /// that was split mid-clause, the end-of-turn garble from the #56 audit.
    fn flush_prose(&mut self) -> Vec<Chunk> {
        if self.buf.is_empty() {
            self.emitted = 0;
            return Vec::new();
        }
        let start = self.emitted.min(self.buf.len());
        let buf = std::mem::take(&mut self.buf);
        self.emitted = 0;
        // The tail is the remainder (no sentence left in it, so this is one
        // chunk per line) or text held for a table, which still splits (#303).
        Self::sentences_of(&stabilize_ordinals(&buf)[start..], false).0
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: Vec<Chunk>) -> Vec<Option<String>> {
        v.into_iter()
            .map(|c| match c {
                Chunk::Text(s) | Chunk::Code { summary: s, .. } => Some(s),
                Chunk::ParagraphBreak => None,
            })
            .collect()
    }

    #[test]
    fn sentence_waits_for_whitespace() {
        let mut a = ProseAssembler::new();
        assert!(texts(a.feed("Version 3.", 0, false)).is_empty());
        assert_eq!(
            texts(a.feed("14 is out. Next", 1, true)),
            // Said as a version since #294.
            vec![
                Some("Version three point fourteen is out.".into()),
                Some("Next".into())
            ]
        );
    }

    #[test]
    fn duplicate_index_zero_starts_a_new_block() {
        let mut a = ProseAssembler::new();
        a.feed("Old text", 0, false);
        let out = texts(a.feed("New.", 0, true));
        assert_eq!(out, vec![Some("Old text".into()), Some("New.".into())]);
    }

    #[test]
    fn split_lines_matches_python() {
        assert_eq!(
            split_lines("a\r\nb\rc\u{2028}d\n"),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn unicode_text_never_panics() {
        let mut a = ProseAssembler::new();
        for (i, d) in ["🚀 “Hi”. ", "日本語。 ", "```é\nx\n``", "`\nend → ok"]
            .iter()
            .enumerate()
        {
            let _ = a.feed(d, i as u32, i == 3);
        }
    }

    #[test]
    fn long_input_never_panics() {
        for s in crate::text::tests::long_inputs() {
            let mut a = ProseAssembler::new();
            let _ = a.feed(&s, 0, false);
            let _ = a.feed("", 1, true);
        }
    }
}
