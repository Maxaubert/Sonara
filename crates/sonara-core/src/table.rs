//! Markdown tables read row by row (#303): each row becomes one sentence
//! with every value after its column name, "Fund KLP Global, share of funds
//! 16.4%, return +24%.", instead of one run of cells.
//!
//! A table is a line with a `|`, then a separator line of dashes, then the
//! rows that follow while they have a `|`. The header is not read on its own.

/// A table found in raw text: its byte span and the rows as sentences.
pub(crate) struct Table {
    pub start: usize,
    pub end: usize,
    pub rows: Vec<String>,
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | ':' | '-' | ' ' | '\t'))
}

/// The cells of one row; outer pipes are optional and `\|` is a literal pipe.
fn cells(line: &str) -> Vec<String> {
    let mut t = line.trim();
    t = t.strip_prefix('|').unwrap_or(t);
    if t.ends_with('|') && !t.ends_with("\\|") {
        t = &t[..t.len() - 1];
    }
    let mut out = vec![String::new()];
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                chars.next();
                out.last_mut().unwrap().push('|');
            }
            '|' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out.iter().map(|c| c.trim().to_string()).collect()
}

/// "Share of funds" reads mid-sentence as "share of funds"; "ETF" stays.
fn mid_sentence(header: &str) -> String {
    let mut chars = header.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => first
            .to_lowercase()
            .chain(header[first.len_utf8()..].chars())
            .collect(),
        _ => header.to_string(),
    }
}

/// One row as a sentence; an empty cell is left out, a cell past the
/// header (or under an empty one) is read without a name.
fn speak_row(header: &[String], row: &[String]) -> String {
    let parts: Vec<String> = row
        .iter()
        .enumerate()
        .filter(|(_, value)| !value.is_empty())
        .map(|(i, value)| match header.get(i).filter(|h| !h.is_empty()) {
            Some(h) if i == 0 => format!("{h} {value}"),
            Some(h) => format!("{} {value}", mid_sentence(h)),
            None => value.clone(),
        })
        .collect();
    let mut s = parts.join(", ");
    if !s.is_empty() && !s.ends_with(['.', '!', '?']) {
        s.push('.');
    }
    s
}

/// The first table in `text`, if any.
pub(crate) fn find(text: &str) -> Option<Table> {
    let mut lines = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        lines.push((at, line));
        at += line.len();
    }
    for i in 0..lines.len().saturating_sub(1) {
        let (start, head) = lines[i];
        let (_, sep) = lines[i + 1];
        if !head.contains('|') || !is_separator(sep) {
            continue;
        }
        let header = cells(head);
        let mut end = lines[i + 1].0 + sep.len();
        let mut rows = Vec::new();
        for &(row_at, line) in &lines[i + 2..] {
            if !line.contains('|') || line.trim().is_empty() {
                break;
            }
            let spoken = speak_row(&header, &cells(line));
            if !spoken.is_empty() {
                rows.push(spoken);
            }
            end = row_at + line.len();
        }
        return Some(Table { start, end, rows });
    }
    None
}

/// Where a table may begin in streamed text: the first line with a `|`
/// that is a header (a separator follows) or may still become one (no
/// complete line follows yet). Text from there on waits until its paragraph
/// is complete, since every row needs the header.
pub(crate) fn hold_from(text: &str) -> usize {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut at = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.contains('|') {
            match lines.get(i + 1) {
                Some(next) if next.ends_with('\n') && !is_separator(next) => {}
                // the separator line may still be arriving
                Some(next) if !next.ends_with('\n') && !is_separator_start(next) => {}
                _ => return at,
            }
        }
        at += line.len();
    }
    text.len()
}

/// A partial line that may still become a separator.
fn is_separator_start(line: &str) -> bool {
    line.chars()
        .all(|c| matches!(c, '|' | ':' | '-' | ' ' | '\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_handle_outer_and_escaped_pipes() {
        assert_eq!(cells("| a | b \\| c |"), vec!["a", "b | c"]);
        assert_eq!(cells("a | b"), vec!["a", "b"]);
        assert_eq!(cells("| a \\|"), vec!["a |"]);
    }

    #[test]
    fn a_separator_needs_a_dash() {
        assert!(is_separator("|:--|--:|"));
        assert!(is_separator("--- | ---"));
        assert!(!is_separator("| | |"));
        assert!(!is_separator("| a |"));
    }

    #[test]
    fn a_header_without_a_separator_is_no_table() {
        assert!(find("| a | b |\n| 1 | 2 |").is_none());
    }

    #[test]
    fn unicode_never_panics() {
        let t = find("| Ålesund | ✓ |\n|---|---|\n| é | 日本 |").unwrap();
        assert_eq!(t.rows, vec!["Ålesund é, ✓ 日本."]);
        assert_eq!(mid_sentence("Ø"), "Ø");
    }
}
