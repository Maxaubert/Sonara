//! Code read aloud (#290, #295): a fenced block's announcement, each line
//! with its label ("Line 3.") and its symbols in words, and inline code in
//! prose with the same words.
//!
//! PURE: no I/O. A line is cut into parts (a run of words, a symbol, a
//! string, a comment, a tag) and the parts are joined with commas, the
//! brief pause between them; `dot` keeps a dotted name in one part
//! ("console dot log"). Identifiers are split into words (`getUserName`,
//! `get_user_name`: "get user name"). Strings, comments and the text
//! between markup tags are read as written.
//!
//! The agent marks each code line [`CODE_LINE`] and a block's announcement
//! [`CODE_START`] (private-use characters, never in real text), so the
//! reader keeps them as their own chunks, reads them at the code speed and
//! pauses after each line (`reader::split_chunks`).

/// Starts a chunk that is one code line.
pub const CODE_LINE: char = '\u{E000}';
/// Starts a chunk that announces a code block.
pub const CODE_START: char = '\u{E001}';

/// What kind of chunk a text is, by its marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Prose,
    Start,
    Line,
}

/// The kind of `chunk` (its first character).
pub fn kind(chunk: &str) -> CodeKind {
    match chunk.chars().next() {
        Some(CODE_LINE) => CodeKind::Line,
        Some(CODE_START) => CodeKind::Start,
        _ => CodeKind::Prose,
    }
}

/// `text` without a leading code marker.
pub fn unmarked(text: &str) -> &str {
    text.strip_prefix([CODE_LINE, CODE_START]).unwrap_or(text)
}

const LANGUAGES: &[(&[&str], &str)] = &[
    (&["ts", "typescript", "mts", "cts"], "TypeScript"),
    (&["tsx"], "T S X"),
    (&["js", "javascript", "mjs", "cjs", "node"], "JavaScript"),
    (&["jsx"], "J S X"),
    (&["py", "python", "python3", "py3"], "Python"),
    (&["rs", "rust"], "Rust"),
    (&["sh", "shell", "console", "shellsession"], "shell"),
    (&["bash"], "Bash"),
    (&["zsh"], "Z shell"),
    (&["ps1", "powershell", "pwsh", "ps"], "PowerShell"),
    (&["cmd", "bat", "batch"], "batch"),
    (&["json", "jsonc", "json5"], "JSON"),
    (&["yaml", "yml"], "YAML"),
    (&["toml"], "TOML"),
    (&["html", "htm"], "HTML"),
    (&["xml", "svg"], "XML"),
    (&["css"], "CSS"),
    (&["scss"], "S C S S"),
    (&["sql", "psql", "mysql", "sqlite"], "S Q L"),
    (&["md", "markdown"], "Markdown"),
    (&["c", "h"], "C"),
    (&["cpp", "c++", "cc", "hpp", "cxx"], "C plus plus"),
    (&["cs", "csharp", "c#"], "C sharp"),
    (&["java"], "Java"),
    (&["kt", "kotlin"], "Kotlin"),
    (&["go", "golang"], "Go"),
    (&["rb", "ruby"], "Ruby"),
    (&["php"], "PHP"),
    (&["swift"], "Swift"),
    (&["lua"], "Lua"),
    (&["r"], "R"),
    (&["vue"], "Vue"),
    (&["svelte"], "Svelte"),
    (&["graphql", "gql"], "GraphQL"),
    (&["dockerfile", "docker"], "Dockerfile"),
    (&["ini", "cfg", "conf"], "config"),
    (&["diff", "patch"], "diff"),
    (&["text", "txt", "plaintext", "plain"], "text"),
];
const MARKUP: &[&str] = &["html", "htm", "xml", "jsx", "tsx", "vue", "svelte", "svg"];
const HASH_COMMENT: &[&str] = &[
    "python",
    "py",
    "python3",
    "py3",
    "bash",
    "sh",
    "shell",
    "zsh",
    "console",
    "ruby",
    "rb",
    "yaml",
    "yml",
    "toml",
    "powershell",
    "ps1",
    "pwsh",
    "ps",
    "r",
    "perl",
    "pl",
    "dockerfile",
    "docker",
    "ini",
    "cfg",
    "conf",
    "makefile",
    "make",
];
const DASH_COMMENT: &[&str] = &["sql", "psql", "mysql", "sqlite", "lua", "haskell", "hs"];
const SHELL: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "shell",
    "console",
    "shellsession",
    "powershell",
    "ps1",
    "pwsh",
    "ps",
    "cmd",
    "bat",
    "batch",
];

/// The first word of a fence info string, lowercased.
fn info_word(info: &str) -> String {
    info.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['{', '}', '.'])
        .to_ascii_lowercase()
}

/// The spoken name of a fence's language: a known name, else the info
/// string's first word as written; `None` without one.
pub fn language_name(info: &str) -> Option<String> {
    let raw = info.split_whitespace().next()?;
    let word = info_word(info);
    Some(
        LANGUAGES
            .iter()
            .find(|(ids, _)| ids.contains(&word.as_str()))
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| raw.to_string()),
    )
}

/// "TypeScript code block, 7 lines." / "Code block, 1 line."
pub fn announcement(info: &str, lines: usize) -> String {
    let size = if lines == 1 {
        "1 line".to_string()
    } else {
        format!("{lines} lines")
    };
    match language_name(info) {
        Some(name) => format!("{name} code block, {size}."),
        None => format!("Code block, {size}."),
    }
}

/// "End of TypeScript code." / "End of code."
pub fn end_of_code(info: &str) -> String {
    match language_name(info) {
        Some(name) => format!("End of {name} code."),
        None => "End of code.".to_string(),
    }
}

/// Source line `n` (1-based) of a block in `info`'s language, as read
/// aloud: "Line 3. ..." ending as a sentence; `None` when nothing in it is
/// spoken (blank, or a lone semicolon).
pub fn line(raw: &str, info: &str, n: usize) -> Option<String> {
    let lang = info_word(info);
    let mut code = raw.trim();
    if let Some(s) = code.strip_suffix(';') {
        code = s.trim_end();
    }
    if code.is_empty() {
        return None;
    }
    let mode = Mode::of(&lang, code);
    let spoken = render(code, &mode);
    if spoken.is_empty() {
        return None;
    }
    let mut out = format!("Line {n}. {spoken}");
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    Some(out)
}

/// Inline code in prose, read with the same words (no label, no final
/// period).
pub fn inline(code: &str) -> String {
    let code = code.trim();
    let mode = Mode::of("", code);
    render(code, &mode)
}

/// How a line is read in its language.
struct Mode {
    markup: bool,
    flags: bool,
    hash_comment: bool,
    dash_comment: bool,
    slash_comment: bool,
}

impl Mode {
    fn of(lang: &str, code: &str) -> Mode {
        let unknown = lang.is_empty();
        let python = matches!(lang, "py" | "python" | "python3" | "py3");
        Mode {
            markup: MARKUP.contains(&lang) || (unknown && looks_like_markup(code)),
            flags: SHELL.contains(&lang) || unknown,
            hash_comment: HASH_COMMENT.contains(&lang),
            dash_comment: DASH_COMMENT.contains(&lang),
            slash_comment: !python,
        }
    }
}

/// A tag such as `<div>`, `</p>` or `<Welcome />`.
fn looks_like_markup(code: &str) -> bool {
    let b = code.as_bytes();
    b.windows(2)
        .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || w[1] == b'/'))
        && code.contains('>')
}

/// Symbols with their words, longest first.
const SYMBOLS: &[(&str, &str)] = &[
    ("===", "strictly equals"),
    ("!==", "not strictly equals"),
    ("...", "dot dot dot"),
    ("**=", "double star equals"),
    ("<<=", "shift left equals"),
    (">>=", "shift right equals"),
    ("=>", "arrow"),
    ("->", "arrow"),
    ("<-", "left arrow"),
    ("==", "double equals"),
    ("!=", "not equals"),
    ("<=", "less or equal"),
    (">=", "greater or equal"),
    ("&&", "and"),
    ("||", "or"),
    ("++", "plus plus"),
    ("--", "minus minus"),
    ("+=", "plus equals"),
    ("-=", "minus equals"),
    ("*=", "star equals"),
    ("/=", "slash equals"),
    ("%=", "percent equals"),
    ("|=", "pipe equals"),
    ("&=", "ampersand equals"),
    (":=", "colon equals"),
    ("::", "double colon"),
    ("??", "double question mark"),
    ("?.", "question dot"),
    ("**", "double star"),
    ("<<", "shift left"),
    (">>", "shift right"),
    ("|>", "pipe greater than"),
    ("..", "dot dot"),
    ("()", "empty parens"),
    ("[]", "empty brackets"),
    ("{}", "empty braces"),
    ("(", "open paren"),
    (")", "close paren"),
    ("[", "open bracket"),
    ("]", "close bracket"),
    ("{", "open brace"),
    ("}", "close brace"),
    ("<", "less than"),
    (">", "greater than"),
    ("=", "equals"),
    ("+", "plus"),
    ("-", "minus"),
    ("*", "star"),
    ("/", "slash"),
    ("%", "percent"),
    ("!", "not"),
    ("?", "question mark"),
    (":", "colon"),
    (";", "semicolon"),
    (",", "comma"),
    ("&", "ampersand"),
    ("|", "pipe"),
    ("^", "caret"),
    ("~", "tilde"),
    ("@", "at"),
    ("#", "hash"),
    ("$", "dollar"),
    ("\\", "backslash"),
    ("'", "tick"),
    ("\"", "quote"),
    ("`", "backtick"),
];

/// Builds the spoken parts of a line.
#[derive(Default)]
struct Parts {
    done: Vec<String>,
    /// The run of words being built.
    words: String,
    /// A `.` after the last word: the next word joins with "dot".
    dot: bool,
}

impl Parts {
    fn word(&mut self, w: &str) {
        if w.is_empty() {
            return;
        }
        if self.dot && !self.words.is_empty() {
            self.words.push_str(" dot ");
        } else if !self.words.is_empty() {
            self.words.push(' ');
        }
        self.dot = false;
        self.words.push_str(w);
    }

    fn dot(&mut self) {
        if self.words.is_empty() || self.dot {
            self.part("dot");
        } else {
            self.dot = true;
        }
    }

    fn flush(&mut self) {
        if self.dot {
            self.words.push_str(" dot");
            self.dot = false;
        }
        if !self.words.is_empty() {
            self.done.push(std::mem::take(&mut self.words));
        }
    }

    /// A part of its own (a symbol, a string, a comment, a tag).
    fn part(&mut self, p: &str) {
        self.flush();
        let p = p.trim();
        if !p.is_empty() {
            self.done.push(p.to_string());
        }
    }

    fn finish(mut self) -> String {
        self.flush();
        self.done.join(", ")
    }
}

/// An identifier as words: snake_case and camelCase split, pieces of a
/// split name lowercased unless they are all capitals ("HTTP server").
pub fn identifier(word: &str) -> String {
    let mut pieces: Vec<String> = Vec::new();
    for chunk in word.split(['_', '-']).filter(|c| !c.is_empty()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut cur = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let prev = i.checked_sub(1).map(|j| chars[j]);
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                Some(p) if c.is_uppercase() => {
                    p.is_lowercase()
                        || p.is_ascii_digit()
                        || (p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
                }
                _ => false,
            };
            if boundary && !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur));
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            pieces.push(cur);
        }
    }
    if pieces.len() <= 1 {
        return pieces.pop().unwrap_or_default();
    }
    pieces
        .iter()
        .map(|p| {
            if p.chars().filter(|c| c.is_alphabetic()).count() > 1
                && p.chars().all(|c| !c.is_lowercase())
            {
                p.clone()
            } else {
                p.to_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The end (byte index) of the quoted string starting at `start` (its
/// opening quote), or `None` when it does not close on this line.
fn string_end(s: &str, start: usize, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s[start + quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return Some(start + quote.len_utf8() + i);
        }
    }
    None
}

/// Text read as written: whitespace collapsed.
fn words_of(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A template literal's text, its `${...}` read as "insert ...".
fn template(content: &str) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let close = after.find('}').unwrap_or(after.len());
        out.push_str(&format!(" insert {} ", inline(&after[..close])));
        rest = after.get(close + 1..).unwrap_or("");
    }
    out.push_str(rest);
    words_of(&out)
}

/// The byte index of the `>` that ends the tag opened at `start`, skipping
/// quoted attribute values and `{...}` expressions.
fn tag_end(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (i, c) in s[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => depth = depth.saturating_sub(1),
            (None, '>') if depth == 0 => return Some(start + i),
            _ => {}
        }
    }
    None
}

/// A tag name at `s[at..]`: its end.
fn name_end(s: &str, at: usize) -> usize {
    s[at..]
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '.' | ':' | '-' | '_')))
        .map_or(s.len(), |(i, _)| at + i)
}

/// The parts of `code`, joined with commas.
fn render(code: &str, mode: &Mode) -> String {
    let mut p = Parts::default();
    tokens(code, mode, &mut p);
    p.finish()
}

fn tokens(s: &str, mode: &Mode, p: &mut Parts) {
    let mut i = 0;
    // A flag can start here: the start or after whitespace.
    let mut at_word = true;
    while i < s.len() {
        let rest = &s[i..];
        let c = rest.chars().next().expect("not at the end");
        if c.is_whitespace() {
            i += c.len_utf8();
            at_word = true;
            continue;
        }
        let was_word = at_word;
        at_word = false;
        // Comments: the rest of the line read as words.
        let comment = (mode.slash_comment && rest.starts_with("//"))
            || (mode.hash_comment && c == '#' && was_word)
            || (mode.dash_comment && rest.starts_with("--"));
        if comment {
            let text = rest.trim_start_matches(['/', '#', '-']);
            p.part("comment");
            p.part(&words_of(text));
            return;
        }
        if let Some(body) = rest.strip_prefix("/*") {
            let end = body.find("*/").map_or(rest.len(), |e| e + 4);
            let inner = rest[2..end.min(rest.len())].trim_end_matches("*/");
            p.part("comment");
            p.part(&words_of(inner.trim_matches('*')));
            i += end;
            continue;
        }
        if mode.markup {
            if let Some(n) = markup(s, i, p) {
                i = n;
                continue;
            }
        }
        // Strings.
        if matches!(c, '"' | '\'' | '`') {
            if let Some(end) = string_end(s, i, c) {
                let content = &s[i + 1..end];
                let spoken = match c {
                    '`' => format!("backtick {} end backtick", template(content)),
                    _ => format!("quote {} end quote", words_of(content)),
                };
                p.part(&words_of(&spoken));
                i = end + 1;
                continue;
            }
        }
        // Flags: -g, --save-dev.
        if mode.flags && was_word && c == '-' {
            let dashes = rest.len() - rest.trim_start_matches('-').len();
            let after = rest[dashes..].chars().next();
            if dashes <= 2 && after.is_some_and(|a| a.is_alphabetic()) {
                let end = rest[dashes..]
                    .char_indices()
                    .find(|(_, ch)| !(ch.is_alphanumeric() || matches!(ch, '-' | '_')))
                    .map_or(rest.len(), |(j, _)| dashes + j);
                let name = rest[dashes..end].replace(['-', '_'], " ");
                let dash = if dashes == 2 { "dash dash" } else { "dash" };
                p.part(&format!("{dash} {}", name.to_lowercase()));
                i += end;
                continue;
            }
        }
        // Numbers: digits, with dots, hex and separators.
        if c.is_ascii_digit() {
            let end = rest
                .char_indices()
                .find(|(j, ch)| {
                    !(ch.is_alphanumeric()
                        || *ch == '_'
                        || (*ch == '.'
                            && rest[j + 1..]
                                .chars()
                                .next()
                                .is_some_and(|d| d.is_ascii_digit())))
                })
                .map_or(rest.len(), |(j, _)| j);
            p.word(&rest[..end]);
            i += end;
            continue;
        }
        if is_word_start(c) {
            let end = rest
                .char_indices()
                .find(|(_, ch)| !is_word_char(*ch))
                .map_or(rest.len(), |(j, _)| j);
            p.word(&identifier(&rest[..end]));
            i += end;
            continue;
        }
        if c == '.' && !rest.starts_with("..") {
            p.dot();
            i += 1;
            continue;
        }
        match SYMBOLS.iter().find(|(sym, _)| rest.starts_with(sym)) {
            Some((sym, word)) => {
                p.part(word);
                i += sym.len();
            }
            None => {
                // Anything else (an emoji, a letter-like symbol): as is.
                p.word(&c.to_string());
                i += c.len_utf8();
            }
        }
    }
}

/// A markup construct at `s[i..]` (a tag, a fragment, a comment), spoken
/// into `p`; the index after it, or `None` when there is none here.
fn markup(s: &str, i: usize, p: &mut Parts) -> Option<usize> {
    let rest = &s[i..];
    if let Some(after) = rest.strip_prefix("<!--") {
        let end = after.find("-->").map_or(rest.len(), |e| e + 4 + 3);
        p.part("comment");
        p.part(&words_of(after.split("-->").next().unwrap_or("")));
        return Some(i + end.min(rest.len()));
    }
    if rest.starts_with("</>") {
        p.part("close fragment");
        return Some(content(s, i + 3, p));
    }
    if rest.starts_with("<>") {
        p.part("open fragment");
        return Some(content(s, i + 2, p));
    }
    if rest.starts_with("/>") {
        p.part("self-closing");
        return Some(content(s, i + 2, p));
    }
    if let Some(after) = rest.strip_prefix("</") {
        if after.starts_with(|c: char| c.is_alphabetic()) {
            let start = i + 2;
            let end = name_end(s, start);
            p.part(&format!("close tag {}", &s[start..end]));
            let close = s[end..].find('>').map_or(s.len(), |e| end + e + 1);
            return Some(content(s, close, p));
        }
    }
    if let Some(after) = rest.strip_prefix('<') {
        if after.starts_with(|c: char| c.is_alphabetic()) {
            let start = i + 1;
            let end = name_end(s, start);
            let name = &s[start..end];
            let Some(close) = tag_end(s, end) else {
                // The tag goes on past this line.
                p.part(&format!("open tag {name}"));
                return Some(end);
            };
            let inner = s[end..close].trim_end();
            let self_closing = inner.ends_with('/');
            let attrs = inner.trim_end_matches('/');
            p.part(&if self_closing {
                format!("tag {name}")
            } else {
                format!("open tag {name}")
            });
            let attr_mode = Mode {
                markup: false,
                flags: false,
                hash_comment: false,
                dash_comment: false,
                slash_comment: false,
            };
            tokens(attrs, &attr_mode, p);
            if self_closing {
                p.part("self-closing");
            }
            return Some(content(s, close + 1, p));
        }
    }
    None
}

/// The text after a tag up to the next `<` or `{`, read as written; the
/// index where it stops.
fn content(s: &str, from: usize, p: &mut Parts) -> usize {
    let from = from.min(s.len());
    let rest = &s[from..];
    let end = rest.find(['<', '{']).map_or(s.len(), |e| from + e);
    let text = words_of(&s[from..end]);
    if !text.is_empty() {
        p.part(&text);
    }
    end
}
