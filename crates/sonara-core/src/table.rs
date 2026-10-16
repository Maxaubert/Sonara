//! Markdown tables read row by row (#303): each row is one line with every
//! value after its column name, "Fund: KLP Global. Share of funds: 16.4%.
//! Return: +24%." (#313), instead of one run of cells.
//!
//! A table is a line with a `|`, then a separator line of dashes with as
//! many cells, then at least one row; the rows go on while they have a `|`.
//! The header is not read on its own (without rows it is read as prose).

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

/// `sep` is the separator under `head`: as many cells, as GFM asks (#25),
/// so a lone `---` under a line with a pipe is a rule, not a table.
fn separates(head: &str, sep: &str) -> bool {
    is_separator(sep) && cells(sep).len() == cells(head).len()
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

/// One row as one line: each value after its column name and a colon, and
/// a full stop after each value (#313: "Fund: KLP Global. Share of funds:
/// 16.4%." pauses between a name and its value, where "Fund KLP Global,
/// share of funds 16.4%" ran them together). An empty cell is left out, a
/// cell past the header (or under an empty one) is read without a name.
fn speak_row(header: &[String], row: &[String]) -> String {
    row.iter()
        .enumerate()
        .filter(|(_, value)| !value.is_empty())
        .map(|(i, value)| {
            let mut part = match header.get(i).filter(|h| !h.is_empty()) {
                Some(h) => format!("{h}: {value}"),
                None => value.clone(),
            };
            if !part.ends_with(['.', '!', '?']) {
                part.push('.');
            }
            part
        })
        .collect::<Vec<_>>()
        .join(" ")
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
        if !head.contains('|') || !separates(head, sep) {
            continue;
        }
        let header = cells(head);
        let body = lines[i + 1].0 + sep.len();
        let mut end = body;
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
        // Without a row the header is the only text: read it as prose
        // rather than drop it (#25).
        if end == body {
            continue;
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
                Some(next) if next.ends_with('\n') && !separates(line, next) => {}
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
        assert_eq!(t.rows, vec!["Ålesund: é. ✓: 日本."]);
    }
}
