//! Code read aloud (#290, #295): a fenced block's announcement, each line
//! with its label ("Line three.") and its symbols in words, and inline code in
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
/// Starts a text already in its spoken form (the agent's prose, #294
/// review): the reader cuts it into sentences and reads it as it is,
/// without running the text rules a second time.
pub const SPOKEN: char = '\u{E002}';

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
    text.strip_prefix([CODE_LINE, CODE_START, SPOKEN])
        .unwrap_or(text)
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
/// Languages whose lines are text: read as written (#295 review).
const PROSE: &[&str] = &["text", "txt", "plaintext", "plain", "md", "markdown"];
/// Languages where a hyphen between letters joins a name (`font-size`).
const KEBAB: &[&str] = &[
    "css",
    "scss",
    "sass",
    "less",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "dockerfile",
    "docker",
    "makefile",
    "make",
];
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
/// aloud: "Line three. ..." ending as a sentence; `None` when nothing in it is
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
    let mode = Mode::of(&lang, code, false);
    let spoken = if mode.prose {
        words_of(code)
    } else {
        render(code, &mode)
    };
    if spoken.is_empty() {
        return None;
    }
    // The number in words: "Line 1." ran into the next word in Kokoro
    // ("oned ...", 2026-10-14).
    let mut out = format!("Line {}. {spoken}", crate::numbers::cardinal(n as u64));
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    Some(out)
}

/// Inline code in prose, read with the same words (no label, no final
/// period).
pub fn inline(code: &str) -> String {
    let code = code.trim();
    // A URL is left to the prose rules.
    if code.contains("://") && !code.contains(char::is_whitespace) {
        return code.to_string();
    }
    let mode = Mode::of("", code, true);
    render(code, &mode)
}

/// How a line is read in its language.
struct Mode {
    markup: bool,
    flags: bool,
    hash_comment: bool,
    dash_comment: bool,
    slash_comment: bool,
    block_comment: bool,
    /// Text, read as written.
    prose: bool,
    /// A hyphen between letters joins a name.
    kebab: bool,
    /// `'a` is a lifetime.
    rust: bool,
    /// Inline code in prose: digit-hyphen-digit stays one word (a date).
    inline: bool,
}

impl Mode {
    fn of(lang: &str, code: &str, inline: bool) -> Mode {
        let unknown = lang.is_empty();
        let python = matches!(lang, "py" | "python" | "python3" | "py3");
        let shell = SHELL.contains(&lang);
        let markup = MARKUP.contains(&lang) || (unknown && looks_like_markup(code));
        Mode {
            markup,
            flags: shell || unknown,
            hash_comment: HASH_COMMENT.contains(&lang),
            dash_comment: DASH_COMMENT.contains(&lang),
            slash_comment: !python && !shell,
            block_comment: !python && !shell && !unknown,
            prose: PROSE.contains(&lang),
            kebab: unknown || shell || markup || KEBAB.contains(&lang),
            rust: matches!(lang, "rs" | "rust"),
            inline,
        }
    }

    /// Attribute values inside a tag.
    fn attrs() -> Mode {
        Mode {
            markup: false,
            flags: false,
            hash_comment: false,
            dash_comment: false,
            slash_comment: false,
            block_comment: false,
            prose: false,
            kebab: true,
            rust: false,
            inline: false,
        }
    }
}

/// A `<` can open a tag here: not right after a name or a closing bracket
/// (`Vec<String>`, `i<n` are comparisons and generics).
fn tag_can_start(s: &str, i: usize) -> bool {
    !s[..i]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ')' | ']'))
}

/// A tag such as `<div>`, `</p>` or `<Welcome />`.
fn looks_like_markup(code: &str) -> bool {
    code.char_indices().any(|(i, c)| {
        c == '<'
            && tag_can_start(code, i)
            && code[i + 1..]
                .chars()
                .next()
                .is_some_and(|n| n.is_ascii_alphabetic() || n == '/')
    }) && code.contains('>')
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
    c.is_alphanumeric() || c == '_' || is_combining(c)
}

/// A combining mark (a decomposed accent): part of the word before it.
fn is_combining(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
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
    // Inside a JSX `{...}` expression after markup text: its depth.
    let mut jsx: Option<usize> = None;
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
        // A comment starts a word: `https://x` is no comment.
        let comment = (mode.slash_comment && was_word && rest.starts_with("//"))
            || (mode.hash_comment && c == '#' && was_word)
            || (mode.dash_comment && was_word && rest.starts_with("--"));
        if comment {
            let text = rest.trim_start_matches(['/', '#', '-']);
            p.part("comment");
            p.part(&words_of(text));
            return;
        }
        if mode.block_comment {
            if let Some(body) = rest.strip_prefix("/*") {
                let (inner, len) = match body.find("*/") {
                    Some(e) => (&body[..e], e + 4),
                    None => (body, rest.len()),
                };
                p.part("comment");
                p.part(&words_of(inner.trim_matches('*')));
                i += len;
                continue;
            }
        }
        if mode.markup {
            if let Some(n) = markup(s, i, p) {
                i = n;
                if s[i..].starts_with('{') {
                    jsx = Some(0);
                }
                continue;
            }
        }
        // A JSX expression: its braces, then the text after it as words.
        if let Some(depth) = jsx {
            if c == '{' {
                jsx = Some(depth + 1);
            } else if c == '}' && depth <= 1 {
                p.part("close brace");
                jsx = None;
                i = content(s, i + 1, p);
                if s[i..].starts_with('{') {
                    jsx = Some(0);
                }
                continue;
            } else if c == '}' {
                jsx = Some(depth - 1);
            }
        }
        // A lifetime (`'a`), not a string.
        if mode.rust && c == '\'' {
            let after = &rest[1..];
            let end = after
                .char_indices()
                .find(|(_, ch)| !is_word_char(*ch))
                .map_or(after.len(), |(j, _)| j);
            if end > 0 && !after[end..].starts_with('\'') {
                p.part(&format!("lifetime {}", &after[..end]));
                i += 1 + end;
                continue;
            }
        }
        // Strings; an apostrophe in a word (`don't`) opens none.
        let apostrophe = c == '\''
            && s[..i]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        if matches!(c, '"' | '\'' | '`') && !apostrophe {
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
                    let digit_next = rest[j + ch.len_utf8()..]
                        .chars()
                        .next()
                        .is_some_and(|d| d.is_ascii_digit());
                    !(ch.is_alphanumeric()
                        || *ch == '_'
                        || (*ch == '.' && digit_next)
                        || (*ch == '-' && mode.inline && digit_next))
                })
                .map_or(rest.len(), |(j, _)| j);
            p.word(&rest[..end]);
            i += end;
            continue;
        }
        if is_word_start(c) {
            // A hyphen (in kebab languages) or an apostrophe between letters
            // stays in the name: `font-size`, `don't`.
            let chars: Vec<(usize, char)> = rest.char_indices().collect();
            let end = chars
                .iter()
                .enumerate()
                .find(|(k, (_, ch))| {
                    let joins = (*ch == '-' && mode.kebab) || *ch == '\'';
                    let between = *k > 0
                        && chars[k - 1].1.is_alphanumeric()
                        && chars.get(k + 1).is_some_and(|(_, n)| n.is_alphabetic());
                    !(is_word_char(*ch) || (joins && between))
                })
                .map_or(rest.len(), |(_, (j, _))| *j);
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
            None if is_combining(c) => {
                // A stray accent joins the word before it.
                p.words.push(c);
                i += c.len_utf8();
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
    if !tag_can_start(s, i) {
        return None;
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
            tokens(attrs, &Mode::attrs(), p);
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
    // Code after a tag (`(<App />)`) is read as code: text is words or
    // sentence punctuation.
    let is_text = text.chars().any(char::is_alphanumeric)
        || (!text.is_empty() && text.chars().all(|c| matches!(c, '.' | '!' | '?')));
    if !is_text {
        return from;
    }
    // The comma between parts stands for a trailing `,` or `;`.
    let text = text.trim_end_matches([',', ';']);
    if !text.is_empty() {
        p.part(text);
    }
    end
}
