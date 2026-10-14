//! Prose pronunciation (#294): dates, times, numbers, money, units,
//! versions, paths, URLs, acronyms and tech terms rewritten as the words
//! people say, before any engine reads them.
//!
//! PURE except for the user's word list ([`set_user_words`], a process-wide
//! setting). One ordered pass of rules, the unambiguous ones first (the
//! order of the survey `research/sonara/2026-10-13-text-normalization-
//! survey.md`, section 7). Every rule writes words only, with no digits,
//! hyphens or symbols, so a later rule (or an engine's own text rules)
//! never reads its output again; whole numbers under 1000 that belong to no
//! class stay as digits, which every engine reads the same way. Inline code
//! and markdown link targets are left alone (code is read by `code_speech`).
//!
//! Sources: NVIDIA NeMo-text-processing (classes and test inputs,
//! Apache-2.0), Festival's acronym rule (spell a capitalised word with no
//! vowel, or with no consonant-vowel-consonant run and under five letters),
//! the tech-term pronunciations cited in the survey. Day-first dates, "the
//! twelfth of December", are the owner's choice (2026-10-13).
use crate::code_speech;
use crate::numbers::{cardinal, cardinal_str, decimal, ordinal, plural, spell_digits, year};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use std::collections::HashMap;
use std::sync::RwLock;

/// The user's own pronunciations, matched first (whole words, case
/// sensitive): (written, said).
static USER: Lazy<RwLock<UserWords>> = Lazy::new(|| RwLock::new(UserWords::default()));

/// The user's words and one pattern for all of them (longest first), so a
/// word is matched once and a `say` is never read by another entry.
#[derive(Default)]
struct UserWords {
    words: Vec<(String, String)>,
    pattern: Option<Regex>,
}

/// Set the user's pronunciations (the `pronunciations` setting).
pub fn set_user_words(words: Vec<(String, String)>) {
    let words: Vec<(String, String)> = words.into_iter().filter(|(w, _)| !w.is_empty()).collect();
    let mut alts: Vec<&str> = words.iter().map(|(w, _)| w.as_str()).collect();
    alts.sort_by_key(|w| std::cmp::Reverse(w.len()));
    let pattern = (!alts.is_empty()).then(|| {
        re(&alts
            .iter()
            .map(|w| regex::escape(w))
            .collect::<Vec<_>>()
            .join("|"))
    });
    *USER.write().unwrap_or_else(|e| e.into_inner()) = UserWords { words, pattern };
}

/// The user's pronunciations.
pub fn user_words() -> Vec<(String, String)> {
    USER.read().unwrap_or_else(|e| e.into_inner()).words.clone()
}

/// How much of a file path is read (the `paths` setting, 2026-10-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDetail {
    /// Without the common folders (drive, `Users\<name>`, `AppData\Local`
    /// and the like), at most the last three parts. The default.
    Short,
    LastTwo,
    LastThree,
    /// Every folder.
    Full,
}

impl PathDetail {
    pub const NAMES: &'static [&'static str] = &["short", "last_two", "last_three", "full"];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn from_name(s: &str) -> Option<PathDetail> {
        [Self::Short, Self::LastTwo, Self::LastThree, Self::Full]
            .into_iter()
            .find(|d| d.name() == s)
    }
}

static PATHS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Set how much of a path is read.
pub fn set_path_detail(d: PathDetail) {
    PATHS.store(d as u8, std::sync::atomic::Ordering::Relaxed);
}

pub fn path_detail() -> PathDetail {
    match PATHS.load(std::sync::atomic::Ordering::Relaxed) {
        1 => PathDetail::LastTwo,
        2 => PathDetail::LastThree,
        3 => PathDetail::Full,
        _ => PathDetail::Short,
    }
}

/// Leading folders that say nothing about the file: the drive and home
/// folders, the app data folders, the program folders.
fn common_prefix_len(parts: &[&str]) -> usize {
    let mut i = 0;
    while let Some(p) = parts.get(i) {
        let p = p.to_ascii_lowercase();
        i += match p.as_str() {
            "users" | "home" if i + 1 < parts.len() - 1 => 2,
            "appdata"
            | "local"
            | "locallow"
            | "roaming"
            | "programdata"
            | "program files"
            | "program files (x86)"
                if i < parts.len() - 1 =>
            {
                1
            }
            // Git Bash and WSL drives: `/c/Users/...`, `/mnt/c/...`.
            "mnt" | "c" | "d" if i == 0 && parts.len() > 2 => 1,
            _ if i == 1 && parts[0].eq_ignore_ascii_case("mnt") && p.len() == 1 => 1,
            _ => break,
        };
    }
    i
}

/// The parts of a path that are read, by `detail`.
fn path_tail<'a>(parts: &'a [&'a str], detail: PathDetail) -> &'a [&'a str] {
    let keep = |n: usize| &parts[parts.len().saturating_sub(n)..];
    match detail {
        PathDetail::Full => parts,
        PathDetail::LastTwo => keep(2),
        PathDetail::LastThree => keep(3),
        PathDetail::Short => {
            let rest = &parts[common_prefix_len(parts)..];
            &rest[rest.len().saturating_sub(3)..]
        }
    }
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid pattern")
}

/// Replace the matches `f` accepts; a match it refuses stays as written.
fn each(
    t: &str,
    re: &Regex,
    mut f: impl FnMut(&Captures, usize, usize) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(t.len());
    let mut last = 0;
    for c in re.captures_iter(t) {
        let m = c.get(0).expect("whole match");
        if let Some(r) = f(&c, m.start(), m.end()) {
            out.push_str(&t[last..m.start()]);
            out.push_str(&r);
            last = m.end();
        }
    }
    out.push_str(&t[last..]);
    out
}

fn prev(t: &str, i: usize) -> Option<char> {
    t[..i].chars().next_back()
}

fn next(t: &str, i: usize) -> Option<char> {
    t[i..].chars().next()
}

fn wordy(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// `t[s..e]` stands alone: no word character on either side, and no
/// `.5` / `,5` / `-5` / `/5` running on after it.
fn free(t: &str, s: usize, e: usize) -> bool {
    if wordy(prev(t, s)) || wordy(next(t, e)) {
        return false;
    }
    let after = &t[e..];
    let mut ch = after.chars();
    !matches!((ch.next(), ch.next()), (Some('.' | ',' | '-' | '/' | ':'), Some(d)) if d.is_ascii_digit())
}

/// Like `free`, and not inside a dotted or slashed token on the left.
fn free_left(t: &str, s: usize, e: usize) -> bool {
    free(t, s, e) && !matches!(prev(t, s), Some('.' | '/' | '\\' | '-' | ':' | '$' | '#'))
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn month_of(name: &str) -> Option<usize> {
    let n = name.to_ascii_lowercase();
    let n = n.trim_end_matches('.');
    MONTHS.iter().position(|m| {
        let m = m.to_ascii_lowercase();
        m == n || (n.len() >= 3 && m.starts_with(n) && (n.len() == 3 || n == "sept"))
    })
}

fn days_in(month: usize, y: u64) -> u64 {
    match month {
        1 => {
            if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) {
                29
            } else {
                28
            }
        }
        3 | 5 | 8 | 10 => 30,
        _ => 31,
    }
}

/// "the twelfth of December twenty twenty six"; `None` for no such day.
fn date_words(day: u64, month: usize, y: Option<u64>) -> Option<String> {
    if month >= 12 || day == 0 || day > days_in(month, y.unwrap_or(2000)) {
        return None;
    }
    let mut s = format!("the {} of {}", ordinal(day), MONTHS[month]);
    if let Some(y) = y {
        s.push(' ');
        s.push_str(&year(y));
    }
    Some(s)
}

/// "fourteen thirty", "nine oh five", "fourteen hundred", "nine o'clock".
fn clock(h: u64, m: u64) -> String {
    let mins = match m {
        0 if (1..=12).contains(&h) => "o'clock".to_string(),
        0 => "hundred".to_string(),
        1..=9 => format!("oh {}", cardinal(m)),
        _ => cardinal(m),
    };
    format!("{} {mins}", cardinal(h))
}

/// Letters spaced ("H T T P"), digits as words.
fn spell(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| match c.to_digit(10) {
            Some(d) => cardinal(u64::from(d)),
            None => c.to_uppercase().to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tech terms whose written form holds a symbol: exact text.
const TECH_SYMBOLS: &[(&str, &str)] = &[
    ("C++", "C plus plus"),
    ("C#", "C sharp"),
    ("F#", "F sharp"),
    ("Q#", "Q sharp"),
    (".NET", "dot net"),
    ("ASP.NET", "A S P dot net"),
    ("Node.js", "node J S"),
    ("Vue.js", "view J S"),
    ("Next.js", "next J S"),
    ("Nuxt.js", "nuxt J S"),
    ("Express.js", "express J S"),
    ("Wi-Fi", "why fy"),
    ("Tcl/Tk", "tickle T K"),
    ("CI/CD", "C I C D"),
    ("UTF-8", "U T F eight"),
    ("UTF-16", "U T F sixteen"),
    ("OS X", "O S ten"),
    ("VS Code", "V S code"),
    ("I/O", "I O"),
    ("TCP/IP", "T C P I P"),
    ("A/B", "A B"),
];

/// Tech terms said other than letter by letter or as the plain word
/// (survey section 5.1; owner's choice SQL "sequel"). Exact case.
static TECH: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    HashMap::from([
        ("SQL", "sequel"),
        ("MySQL", "my S Q L"),
        ("PostgreSQL", "post gres Q L"),
        ("Postgres", "post gres"),
        ("SQLite", "sequel lite"),
        ("JSON", "jay son"),
        ("YAML", "yammel"),
        ("TOML", "tommel"),
        ("XAML", "zammel"),
        ("SAML", "sammel"),
        ("Redis", "red iss"),
        ("etcd", "et see dee"),
        ("AJAX", "ay jax"),
        ("Ajax", "ay jax"),
        ("JWT", "jot"),
        ("GUI", "gooey"),
        ("nginx", "engine X"),
        ("NGINX", "engine X"),
        ("Nginx", "engine X"),
        ("sudo", "soo doo"),
        ("kubectl", "kube control"),
        ("Kubernetes", "koo ber net eez"),
        ("K8s", "koo ber net eez"),
        ("k8s", "koo ber net eez"),
        ("npm", "N P M"),
        ("pnpm", "P N P M"),
        ("PyPI", "pie P I"),
        ("PyPy", "pie pie"),
        ("NumPy", "num pie"),
        ("SciPy", "sigh pie"),
        ("Jupyter", "Jupiter"),
        ("Django", "jang go"),
        ("Vue", "view"),
        ("Vite", "veet"),
        ("Deno", "dee no"),
        ("NuGet", "new get"),
        ("Qt", "cute"),
        ("systemd", "system D"),
        ("CAPTCHA", "cap cha"),
        ("WYSIWYG", "wizzy wig"),
        ("OCaml", "oh camel"),
        ("Lua", "loo uh"),
        ("Tcl", "tickle"),
        ("LaTeX", "lah tek"),
        ("TeX", "tek"),
        ("regex", "reg ex"),
        ("enum", "ee num"),
        ("tuple", "too pull"),
        ("async", "ay sink"),
        ("GNU", "guh noo"),
        ("GNOME", "guh nome"),
        ("Ubuntu", "oo boon too"),
        ("SUSE", "soo zuh"),
        ("openSUSE", "open soo zuh"),
        ("CentOS", "sent oss"),
        ("Xfce", "X F C E"),
        ("POSIX", "pozz icks"),
        ("iOS", "eye O S"),
        ("macOS", "mac O S"),
        ("JPEG", "jay peg"),
        ("JPG", "jay peg"),
        ("ASCII", "ass key"),
        ("SCSI", "scuzzy"),
        ("IEEE", "I triple E"),
        ("WiFi", "why fy"),
        ("SaaS", "sass"),
        ("PaaS", "pass"),
        ("IaaS", "eye as"),
        ("BIOS", "bye oss"),
        ("MIDI", "middy"),
        ("MPEG", "em peg"),
        ("OLED", "oh led"),
        ("ASIC", "ay sick"),
        ("IMAP", "eye map"),
        ("VRAM", "vee ram"),
        ("DRAM", "dee ram"),
        ("SRAM", "ess ram"),
        ("EEPROM", "E E prom"),
        ("MOSFET", "moss fet"),
        ("RISC", "risk"),
        ("FIFO", "fy foe"),
        ("LIFO", "ly foe"),
        ("NVIDIA", "en vidia"),
        ("Nvidia", "en vidia"),
        ("COBOL", "co ball"),
        ("HVAC", "aitch vac"),
        ("AAA", "triple A"),
        ("DevOps", "dev ops"),
        ("OAuth", "oh auth"),
        ("stdin", "standard in"),
        ("stdout", "standard out"),
        ("stderr", "standard error"),
        ("Wasm", "wazzum"),
        ("WASM", "wazzum"),
        ("GraphQL", "graph Q L"),
        ("gRPC", "G R P C"),
        ("CMake", "C make"),
        ("tmux", "tee mux"),
        ("zsh", "Z shell"),
        ("i18n", "internationalization"),
        ("l10n", "localization"),
        ("a11y", "accessibility"),
        ("TODO", "to do"),
        ("OK", "okay"),
    ])
});

/// Capitalised words said as words, never spelled (survey section 5.2).
static KEEP: Lazy<std::collections::HashSet<&'static str>> = Lazy::new(|| {
    [
        "NASA", "NATO", "RAM", "ROM", "LAN", "WAN", "GIF", "RAID", "NAND", "CRUD", "BASIC", "DOS",
        "ANSI", "QWERTY", "CERN", "DARPA", "NASDAQ", "FEMA", "UNICEF", "UNESCO", "OPEC", "AIDS",
        "COVID", "CRISPR", "HIPAA", "ASAP", "AWOL", "FOMO", "YOLO", "EULA", "SIM", "PIN", "VAT",
        "IMAX", "FIFA", "UEFA", "LASER", "RADAR", "SONAR", "SCUBA", "LIDAR", "FAQ", "MIME", "TIFF",
        "WIP", "README",
    ]
    .into_iter()
    .collect()
});

/// Capitalised English words said as words, though Festival's rule would
/// spell them (emphasis, log levels, keywords; #294 review).
const COMMON: &[&str] = &[
    "NO", "SO", "GO", "DO", "TO", "OR", "IS", "AS", "AT", "IN", "ON", "OF", "BE", "ME", "WE", "HE",
    "BY", "MY", "UP", "AN", "IF", "THE", "AND", "ARE", "ALL", "ANY", "ONE", "OUT", "WHO", "WHY",
    "HOW", "NEW", "OFF", "OLD", "OUR", "YOU", "TOO", "USE", "ONLY", "TRUE", "HEAD", "FAIL", "INFO",
    "NOTE", "EACH", "ALSO", "NEVER", "MUST", "NONE", "OPEN", "DONE", "READ", "SEND", "STOP",
    "SKIP", "WARN", "NULL", "WAIT", "EXIT", "EDIT", "TEST", "FREE", "MORE", "MOST", "YES", "NOT",
    "SEE", "ERROR", "DEBUG", "TRACE", "FATAL", "FALSE", "PASS", "OKAY", "VERY", "WILL", "SHALL",
    "MAY", "IDEA", "AREA", "DATA", "BODY", "KEY", "KEYS", "TYPE", "FILE", "PATH", "HOME", "USER",
    "SAVE", "LOAD", "TODO", "FIXME", "HACK", "XXX", "EOF", "AUTO", "BOTH", "FROM", "INTO", "WITH",
    "THIS", "THAT", "HAVE", "WERE", "WHAT", "WHEN", "HERE", "THERE", "OVER", "UNDER", "ABOUT",
    "AFTER", "BEFORE", "AGAIN", "AWAY", "BACK", "DOWN", "DONT", "CANT", "WONT",
];

/// Spelled even though they make a word (survey: forced letters).
const FORCED: &[&str] = &[
    "OS", "ID", "IT", "CI", "IO", "US", "LED", "MIT", "PNG", "IDs",
];

/// Brand names in mixed case read as they are, never split into words.
const BRANDS: &[&str] = &[
    "GitHub",
    "GitLab",
    "TypeScript",
    "JavaScript",
    "PowerShell",
    "YouTube",
    "LinkedIn",
    "OpenAI",
    "ChatGPT",
    "iPhone",
    "iPad",
    "MacBook",
    "WordPress",
    "PayPal",
    "DeepSeek",
    "PlayStation",
    "BitBucket",
    "DuckDuckGo",
    "WhatsApp",
    "FastAPI",
    "OpenSSL",
    "OpenGL",
    "WebGL",
    "WebSocket",
    "WebAssembly",
    "McDonald",
    "IntelliJ",
    "PyTorch",
    "TensorFlow",
    "SonarQube",
];

/// Festival's rule (`token.cc`): a capitalised word is spelled when it has
/// no vowel, or no consonant-vowel-consonant run and under five letters.
fn spelled(word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    let has_aeiou = w.iter().any(|c| "AEIOU".contains(*c));
    let vowel = |c: char| "AEIOU".contains(c) || (!has_aeiou && c == 'Y');
    if !w.iter().any(|c| vowel(*c)) {
        return true;
    }
    let cvc = w
        .windows(3)
        .any(|x| !vowel(x[0]) && vowel(x[1]) && !vowel(x[2]));
    !cvc && w.len() < 5
}

/// A capitalised word as said: a known term, kept, or letters.
fn caps_word(w: &str) -> String {
    if let Some(t) = TECH.get(w) {
        return (*t).to_string();
    }
    if FORCED.contains(&w) {
        return spell(w);
    }
    if KEEP.contains(w) || COMMON.contains(&w) || w.len() > 5 || w.len() < 2 {
        return w.to_string();
    }
    // A short roman numeral on its own is spelled (`XIV`): no keyword
    // said it is a number.
    if !spelled(w) && !(w.len() <= 4 && roman(w).is_some()) {
        return w.to_string();
    }
    spell(w)
}

/// Every word of two or more letters is in capitals: shouting, read as it
/// is.
fn shouting(t: &str) -> bool {
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| w.chars().count() >= 2)
        .collect();
    words.len() >= 3 && words.iter().all(|w| !w.chars().any(char::is_lowercase))
}

const EXT_WORDS: &[(&str, &str)] = &[
    ("json", "jay son"),
    ("toml", "tommel"),
    ("yaml", "yammel"),
    ("yml", "yammel"),
    ("lock", "lock"),
    ("log", "log"),
    ("zip", "zip"),
    ("tar", "tar"),
    ("wav", "wave"),
    ("jpeg", "jay peg"),
    ("jpg", "jay peg"),
    ("html", "H T M L"),
    ("exe", "exe"),
    ("ini", "I N I"),
];
/// File extensions that make a bare name a file name (`README.md`).
const KNOWN_EXT: &[&str] = &[
    "rs", "md", "json", "toml", "yaml", "yml", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "txt",
    "log", "lock", "exe", "dll", "sh", "ps1", "bat", "cmd", "html", "css", "scss", "c", "h", "cpp",
    "hpp", "cs", "java", "go", "rb", "php", "swift", "kt", "sql", "xml", "csv", "ini", "cfg",
    "env", "png", "jpg", "jpeg", "gif", "svg", "wav", "mp3", "mp4", "zip", "tar", "gz", "pdf",
    "docx", "xlsx", "pptx", "vue", "svelte", "lua", "jsonl", "ipynb", "wasm", "pem", "crt",
];

/// A file extension as said: a word, or its letters.
fn ext_words(ext: &str) -> String {
    let lower = ext.to_ascii_lowercase();
    EXT_WORDS
        .iter()
        .find(|(e, _)| *e == lower)
        .map(|(_, w)| (*w).to_string())
        .unwrap_or_else(|| spell(ext))
}

/// One path part as said: `text.rs` "text dot R S", `.claude` "dot claude".
fn path_part(part: &str) -> String {
    let (dot, rest) = match part.strip_prefix('.') {
        Some(r) => ("dot ", r),
        None => ("", part),
    };
    let words = match rest.rsplit_once('.') {
        Some((name, ext)) if !name.is_empty() && ext.chars().all(char::is_alphanumeric) => {
            format!(
                "{} dot {}",
                name.replace(['-', '_', '.'], " "),
                ext_words(ext)
            )
        }
        _ => rest.replace(['-', '_'], " "),
    };
    format!("{dot}{words}")
}

/// A URL: its host and the last useful path part.
fn url_words(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let host = parts.next().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let mut out = host.split('.').collect::<Vec<_>>().join(" dot ");
    let path: Vec<&str> = parts.collect();
    if let Some(last) = path.last() {
        let tail = if last.chars().all(|c| c.is_ascii_digit()) && path.len() >= 2 {
            format!("{} {}", path[path.len() - 2], cardinal_str(last))
        } else {
            last.replace(['-', '_'], " ")
        };
        let tail = match tail.rsplit_once('.') {
            Some((n, e)) if KNOWN_EXT.contains(&e.to_ascii_lowercase().as_str()) => {
                format!("{n} dot {}", ext_words(e))
            }
            _ => tail,
        };
        out.push_str(", ");
        out.push_str(&tail);
    }
    out
}

/// A roman numeral's value (I to MMMM), if it is one.
fn roman(s: &str) -> Option<u64> {
    let val = |c| match c {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        'D' => Some(500),
        'M' => Some(1000),
        _ => None,
    };
    let vals: Vec<u64> = s.chars().map(val).collect::<Option<_>>()?;
    let mut total = 0;
    for (i, v) in vals.iter().enumerate() {
        if vals.get(i + 1).is_some_and(|n| n > v) {
            total -= *v as i64;
        } else {
            total += *v as i64;
        }
    }
    (total > 0).then_some(total as u64)
}

/// Units after a number: (written, one, more, needs a space before it).
const UNITS: &[(&str, &str, &str, bool)] = &[
    ("km/h", "kilometre per hour", "kilometres per hour", false),
    ("mph", "mile per hour", "miles per hour", false),
    ("Gbps", "gigabit per second", "gigabits per second", false),
    ("Mbps", "megabit per second", "megabits per second", false),
    ("Kbps", "kilobit per second", "kilobits per second", false),
    ("kbps", "kilobit per second", "kilobits per second", false),
    ("°C", "degree Celsius", "degrees Celsius", false),
    ("°F", "degree Fahrenheit", "degrees Fahrenheit", false),
    ("°", "degree", "degrees", false),
    ("km", "kilometre", "kilometres", false),
    ("cm", "centimetre", "centimetres", false),
    ("mm", "millimetre", "millimetres", false),
    ("kg", "kilogram", "kilograms", false),
    ("mg", "milligram", "milligrams", false),
    ("ms", "millisecond", "milliseconds", false),
    ("µs", "microsecond", "microseconds", false),
    ("ns", "nanosecond", "nanoseconds", false),
    ("min", "minute", "minutes", false),
    ("hrs", "hour", "hours", false),
    ("hr", "hour", "hours", false),
    ("h", "hour", "hours", false),
    ("s", "second", "seconds", true),
    ("TB", "terabyte", "terabytes", false),
    ("GB", "gigabyte", "gigabytes", false),
    ("MB", "megabyte", "megabytes", false),
    ("KB", "kilobyte", "kilobytes", false),
    ("kB", "kilobyte", "kilobytes", false),
    ("Gb", "gigabit", "gigabits", false),
    ("Mb", "megabit", "megabits", false),
    ("Kb", "kilobit", "kilobits", false),
    ("GHz", "gigahertz", "gigahertz", false),
    ("MHz", "megahertz", "megahertz", false),
    ("kHz", "kilohertz", "kilohertz", false),
    ("Hz", "hertz", "hertz", false),
    ("kW", "kilowatt", "kilowatts", false),
    ("W", "watt", "watts", false),
    ("V", "volt", "volts", false),
    ("mAh", "milliamp hour", "milliamp hours", false),
    ("px", "pixel", "pixels", false),
    ("pt", "point", "points", false),
    ("dpi", "D P I", "D P I", false),
    ("fps", "frame per second", "frames per second", false),
    ("ml", "millilitre", "millilitres", false),
    ("mL", "millilitre", "millilitres", false),
    ("l", "litre", "litres", true),
    ("L", "litre", "litres", true),
    ("m", "m", "m", false),
];

/// A number as words: digits with separators, a decimal, a sign.
fn number_words(sign: &str, n: &str) -> String {
    let n = n.replace([',', '\u{00A0}', '\u{202F}', '\u{2009}'], "");
    let w = match n.split_once('.') {
        Some((i, f)) => decimal(i, f),
        None => cardinal_str(&n),
    };
    if sign.is_empty() {
        w
    } else {
        format!("minus {w}")
    }
}

/// A sign is a minus only at the start or after a space, `(` or `=`.
fn sign_ok(t: &str, s: usize) -> bool {
    !matches!(prev(t, s), Some(c) if !(c.is_whitespace() || c == '(' || c == '='))
}

/// The rules, in order. Patterns are compiled once.
struct Rules {
    protect: Regex,
    url: Regex,
    email: Regex,
    path: Regex,
    iso: Regex,
    ip_port: Regex,
    ip: Regex,
    host_port: Regex,
    version_v: Regex,
    version_word: Regex,
    version_tool: Regex,
    version_bare: Regex,
    hex: Regex,
    uuid: Regex,
    commit: Regex,
    long_hex: Regex,
    phone_plus: Regex,
    phone_cue: Regex,
    money: Regex,
    nok_before: Regex,
    nok_after: Regex,
    iso_money_after: Regex,
    iso_money_before: Regex,
    measure: Regex,
    percent: Regex,
    times_x: Regex,
    bits: Regex,
    ampm: Regex,
    duration: Regex,
    clock: Regex,
    ratio: Regex,
    date_num: Regex,
    date_short: Regex,
    date_dm: Regex,
    date_md: Regex,
    range: Regex,
    mixed_fraction: Regex,
    fraction: Regex,
    ordinal: Regex,
    decimal: Regex,
    grouped: Regex,
    negative: Regex,
    leading_zero: Regex,
    decade: Regex,
    big: Regex,
    abbrev: Regex,
    dotted_caps: Regex,
    roman: Regex,
    call: Regex,
    flag: Regex,
    hash_num: Regex,
    amp_letters: Regex,
    letters_digits: Regex,
    word: Regex,
    caps_plural: Regex,
    rating: Regex,
    grouped_space: Regex,
}

static RULES: Lazy<Rules> = Lazy::new(|| {
    let months = "January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sept|Sep|Oct|Nov|Dec";
    let units = UNITS
        .iter()
        .map(|u| regex::escape(u.0))
        .collect::<Vec<_>>()
        .join("|");
    Rules {
        protect: re(r"`[^`\n]*`|\]\([^)\s]*\)"),
        url: re(r"https?://[^\s<>()\[\]]+"),
        email: re(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+"),
        path: re(
            r"(?:[A-Za-z]:\\|~[\\/]|\.{1,2}[\\/]|/)?(?:[\p{L}\p{N}_.@-]+[\\/])*[\p{L}\p{N}_@-][\p{L}\p{N}_.@-]*\.[A-Za-z][A-Za-z0-9]{0,6}(?::\d+(?::\d+)?)?",
        ),
        iso: re(
            r"(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?",
        ),
        ip_port: re(r"(\d{1,3}(?:\.\d{1,3}){3}):(\d{1,5})"),
        ip: re(r"(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})"),
        host_port: re(r"localhost:(\d{1,5})"),
        version_v: re(
            r"[vV](\d+(?:\.\d+)*)((?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?)(?:\+[0-9A-Za-z.]+)?",
        ),
        version_word: re(
            r"([Vv]ersion) (\d+(?:\.\d+)+)((?:-[A-Za-z][0-9A-Za-z]*(?:\.[0-9A-Za-z]+)*)?)(?:\+[0-9A-Za-z.]+)?",
        ),
        version_tool: re(
            r"(Python|Node|Rust|Go|Java|Kotlin|Swift|Ruby|PHP|Perl|Windows|Android|Debian|Chrome|Firefox|Safari|PostgreSQL|Postgres|MySQL|Redis|Docker|Electron|React|Angular|Svelte|Deno|Bun|Cargo|TypeScript|Sonara|Claude|Gemini|GPT|Kokoro|Opus|Sonnet|Haiku|Fable|Ubuntu|Fedora|Unity|Unreal|Blender|Godot|Qt|Java|Dotnet|LLVM|GCC|Clang|Excel|Office|Pixel|iPhone) (\d+(?:\.\d+)+)((?:-[A-Za-z][0-9A-Za-z]*(?:\.[0-9A-Za-z]+)*)?)(?:\+[0-9A-Za-z.]+)?",
        ),
        version_bare: re(
            r"(\d+(?:\.\d+){2,})((?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?)(?:\+[0-9A-Za-z.]+)?",
        ),
        hex: re(r"0x([0-9A-Fa-f]+)"),
        uuid: re(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"),
        commit: re(r"([Cc]ommit|SHA|sha|[Hh]ash|[Rr]evision|[Rr]ev) ([0-9a-f]{7,40})"),
        long_hex: re(r"[0-9a-fA-F]{32,64}"),
        phone_plus: re(r"\+(\d{1,3})((?:[ -]\d{2,4}){2,5})"),
        phone_cue: re(
            r"([Tt]el|[Pp]hone|[Rr]ing|[Cc]all|[Mm]obile|[Mm]obil)\.?:? ((?:\d{2,4}[ -]){1,5}\d{2,4})",
        ),
        money: re(
            r"([$£€])(\d[\d,]*(?:\.\d+)?)(?:\s?(bn|million|billion|thousand|trillion|k|K|m|M|b|B)\b)?(?:\s?[-–]\s?[$£€]?(\d[\d,]*(?:\.\d+)?))?",
        ),
        nok_before: re(r"(?:NOK|kr\.?) ?(\d{1,3}(?:[ .]\d{3})+|\d+)(?:,(\d{2}|-))?"),
        nok_after: re(r"(\d{1,3}(?:[ .]\d{3})+|\d+)(?:,(\d{2}|-))? ?(?:kr|NOK)"),
        iso_money_after: re(r"(\d[\d,]*(?:\.\d+)?) ?(USD|EUR|GBP|SEK|DKK|JPY|CHF|CAD|AUD|CNY|INR)"),
        iso_money_before: re(
            r"(USD|EUR|GBP|SEK|DKK|JPY|CHF|CAD|AUD|CNY|INR) ?(\d[\d,]*(?:\.\d+)?)",
        ),
        measure: re(&format!(
            r"(-?)(?:(\d+(?:\.\d+)?) ?[-–] ?)?(\d[\d,]*(?:\.\d+)?)( ?)({units})"
        )),
        percent: re(r"(-?)(\d[\d,]*(?:\.\d+)?) ?%"),
        times_x: re(r"(\d+(?:\.\d+)?)[x×]"),
        bits: re(
            r"(\d+)-(bit|core|inch|byte|day|hour|minute|second|week|month|year|page|step|part|point|line)",
        ),
        ampm: re(r"(\d{1,2})(?::(\d{2}))? ?([AaPp])\.?[Mm](\.?)"),
        duration: re(
            r"(took|elapsed|duration|lasted|after|in|for|Took|Elapsed|Duration) (\d{1,2}):(\d{2}):(\d{2})",
        ),
        clock: re(r"(\d{1,2}):(\d{2})(?::(\d{2}))?"),
        ratio: re(r"(\d{1,2}):(\d)"),
        date_num: re(r"(\d{1,2})([./-])(\d{1,2})([./-])(\d{4})"),
        date_short: re(r"(on|until|due|On|Until|Due) (\d{1,2})/(\d{1,2})"),
        date_dm: re(&format!(
            r"(\d{{1,2}})(?:st|nd|rd|th)? ({months})\.?(?:,? (\d{{4}}))?"
        )),
        date_md: re(&format!(
            r"({months})\.? (\d{{1,2}})(?:st|nd|rd|th)?(?:,? (\d{{4}}))?"
        )),
        range: re(r"(\d{1,4}) ?[-–] ?(\d{1,4})"),
        mixed_fraction: re(r"(\d+) (\d)/(\d{1,2})"),
        fraction: re(r"(\d{1,2})/(\d{1,2})"),
        rating: re(r"(\d+)\.(\d+)/(\d{1,2})"),
        grouped_space: re(r"(-?)(\d{1,3}(?: \d{3})+)"),
        ordinal: re(r"(\d+)(st|nd|rd|th)"),
        decimal: re(r"(-?)(\d*)\.(\d+)"),
        grouped: re(r"(-?)(\d{1,3}(?:(?:,|\u{00A0}|\u{202F}|\u{2009})\d{3})+(?:\.\d+)?)"),
        negative: re(r"-(\d+)"),
        leading_zero: re(r"0\d+"),
        decade: re(r"'?(\d{2,4})'?s"),
        big: re(r"(\d{4,})"),
        abbrev: re(r"e\.g\.|i\.e\.|etc\.|vs\.|Dr\.|Mr\.|Mrs\.|Ms\.|approx\.|no\. (\d)"),
        dotted_caps: re(r"(?:[A-Z]\.){2,}"),
        roman: re(
            r"(Chapter|Part|Phase|Act|Section|Volume|Book|Stage|Level|Season|Episode|War) ([IVXLCDM]+)",
        ),
        call: re(r"([A-Za-z_][A-Za-z0-9_.]*)\(\)"),
        flag: re(r"(--?)([A-Za-z][A-Za-z0-9-]*)"),
        hash_num: re(r"#(\d+)"),
        amp_letters: re(r"([A-Z])&([A-Z])"),
        letters_digits: re(r"([A-Za-z]{1,5})(\d{1,4})"),
        word: re(r"[A-Za-z][A-Za-z0-9_']*"),
        caps_plural: re(r"([A-Z]{2,5})s"),
    }
});

/// `text` with every rule applied (module docs).
pub fn pronounce(text: &str) -> String {
    let r = &*RULES;
    // Inline code, link targets and the user's words wait aside, as
    // placeholders no rule reads (their number in private-use digits).
    let mut kept: Vec<String> = Vec::new();
    let text = text.replace(['\u{E020}', '\u{E021}'], "");
    let mut t = each(&text, &r.protect, |c, _, _| {
        // A file path in inline code is read as a path (2026-10-14).
        let inner = c[0].strip_prefix('`').and_then(|s| s.strip_suffix('`'));
        if let Some(p) = inner.filter(|p| p.contains(['/', '\\'])) {
            if r.path
                .find(p)
                .is_some_and(|m| m.start() == 0 && m.end() == p.len())
            {
                return Some(p.to_string());
            }
        }
        Some(hold(&mut kept, c[0].to_string()))
    });
    t = user(&t, &mut kept);
    t = tech_symbols(&t);
    t = each(&t, &r.url, |c, _, _| {
        let raw = c[0].trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        let tail = &c[0][raw.len()..];
        Some(format!("{}{tail}", url_words(raw)))
    });
    t = each(&t, &r.email, |c, s, e| {
        free(&t, s, e).then(|| {
            let (user, domain) = c[0].split_once('@').expect("an at");
            format!(
                "{} at {}",
                user.replace('.', " dot "),
                domain.split('.').collect::<Vec<_>>().join(" dot ")
            )
        })
    });
    t = paths(&t);
    t = each(&t, &r.iso, |c, s, e| {
        if !free_left(&t, s, e) {
            return None;
        }
        let num = |i: usize| c.get(i).and_then(|m| m.as_str().parse::<u64>().ok());
        let (y, m, d) = (num(1)?, num(2)?, num(3)?);
        let mut w = date_words(d, (m as usize).checked_sub(1)?, Some(y))?;
        if let (Some(h), Some(mi)) = (num(4), num(5)) {
            if h > 23 || mi > 59 {
                return None;
            }
            w.push_str(&format!(" at {}", clock(h, mi)));
            match c.get(7).map(|m| m.as_str()) {
                Some("Z") => w.push_str(" U T C"),
                Some(z) => w.push_str(&format!(" U T C {}", z.replace(':', ""))),
                None => {}
            }
        }
        Some(w)
    });
    t = each(&t, &r.ip_port, |c, s, e| {
        let ip = ip_words(&c[1])?;
        free_left(&t, s, e).then(|| format!("{ip} port {}", spell_digits(&c[2])))
    });
    t = each(&t, &r.ip, |c, s, e| {
        if !free_left(&t, s, e) {
            return None;
        }
        ip_words(&c[0])
    });
    t = each(&t, &r.host_port, |c, s, e| {
        free_left(&t, s, e).then(|| format!("localhost port {}", spell_digits(&c[1])))
    });
    t = versions(&t);
    t = each(&t, &r.uuid, |_, s, e| {
        free(&t, s, e).then(|| "a U U I D".into())
    });
    t = each(&t, &r.hex, |c, s, e| {
        free_left(&t, s, e).then(|| format!("hex {}", spell(&c[1])))
    });
    t = each(&t, &r.commit, |c, s, e| {
        let h = &c[2];
        (free(&t, s, e) && hashy(h)).then(|| format!("{} {}", &c[1], spell(&h[..4])))
    });
    t = each(&t, &r.long_hex, |c, s, e| {
        (free(&t, s, e) && hashy(&c[0])).then(|| "a hash".into())
    });
    t = phones(&t);
    t = money(&t);
    t = measures(&t);
    t = times(&t);
    t = dates(&t);
    t = ranges(&t);
    t = fractions(&t);
    t = numbers(&t);
    t = abbreviations(&t);
    t = symbols(&t);
    t = words(&t);
    // Back in place.
    if kept.is_empty() {
        return t;
    }
    each(&t, &PLACEHOLDER, |c, _, _| {
        let i = c[1].chars().try_fold(0usize, |n, d| {
            Some(n * 10 + (d as u32).checked_sub(0xE030)? as usize)
        })?;
        kept.get(i).cloned()
    })
}

/// Keep `s` aside: a placeholder with its index in private-use digits
/// (U+E030 to U+E039), which no rule reads as a number.
fn hold(kept: &mut Vec<String>, s: String) -> String {
    kept.push(s);
    let digits: String = (kept.len() - 1)
        .to_string()
        .chars()
        .filter_map(|d| char::from_u32(0xE030 + d.to_digit(10)?))
        .collect();
    format!("\u{E020}{digits}\u{E021}")
}

/// Looks like a hash: digits and letters mixed, several different ones.
fn hashy(h: &str) -> bool {
    let digits = h.chars().filter(char::is_ascii_digit).count();
    let letters = h.len() - digits;
    let distinct = h.chars().collect::<std::collections::HashSet<_>>().len();
    let min = (h.len() / 8).max(1);
    digits >= min && letters >= min && distinct >= 5
}

fn ip_words(ip: &str) -> Option<String> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|p| p.parse::<u16>().map_or(true, |n| n > 255))
    {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|p| spell_digits(p))
            .collect::<Vec<_>>()
            .join(" dot "),
    )
}

/// The user's words, each kept aside as its `say` (read as written, by no
/// other rule). A word inside a path, URL or address is left to it.
fn user(t: &str, kept: &mut Vec<String>) -> String {
    let words = USER.read().unwrap_or_else(|e| e.into_inner());
    let Some(pat) = &words.pattern else {
        return t.to_string();
    };
    each(t, pat, |c, s, e| {
        let (p, n) = (prev(t, s), next(t, e));
        let p2 = t[..s].chars().rev().nth(1);
        let n2 = t[e..].chars().nth(1);
        let inside = matches!(p, Some('/' | '\\'))
            || (matches!(p, Some('.' | '@' | ':')) && wordy(p2))
            || matches!(n, Some('/' | '\\'))
            || (matches!(n, Some('.' | ':' | '@')) && wordy(n2));
        if wordy(p) || wordy(n) || inside {
            return None;
        }
        let say = words.words.iter().find(|(w, _)| *w == c[0])?.1.clone();
        Some(hold(kept, say))
    })
}

static TECH_SYMBOL_RES: Lazy<Vec<(&'static str, &'static str, Regex)>> = Lazy::new(|| {
    TECH_SYMBOLS
        .iter()
        .map(|(t, s)| (*t, *s, re(&regex::escape(t))))
        .collect()
});
static PLACEHOLDER: Lazy<Regex> = Lazy::new(|| re("\u{E020}([\u{E030}-\u{E039}]+)\u{E021}"));
static SLASH_WORDS: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| {
    [
        ("n/a", "not applicable"),
        ("N/A", "not applicable"),
        ("and/or", "and or"),
    ]
    .iter()
    .map(|(f, t)| (re(&regex::escape(f)), *t))
    .collect()
});

fn tech_symbols(t: &str) -> String {
    let mut out = t.to_string();
    for (term, say, pat) in TECH_SYMBOL_RES.iter() {
        if !out.contains(term) {
            continue;
        }
        let cur = out.clone();
        out = each(&cur, pat, |_, s, e| {
            let left_ok = !wordy(prev(&cur, s)) || !term.starts_with(|c: char| c.is_alphanumeric());
            let right_ok = !wordy(next(&cur, e)) || !term.ends_with(|c: char| c.is_alphanumeric());
            (left_ok && right_ok && !matches!(prev(&cur, s), Some('.'))).then(|| (*say).to_string())
        });
    }
    out
}

fn paths(t: &str) -> String {
    paths_with(t, path_detail())
}

fn paths_with(t: &str, detail: PathDetail) -> String {
    let r = &*RULES;
    each(t, &r.path, |c, s, e| {
        let raw = &c[0];
        if wordy(prev(t, s)) || wordy(next(t, e)) || matches!(prev(t, s), Some('@' | '.')) {
            return None;
        }
        // `:N` after the file is a line (not the drive's colon).
        let split = raw
            .match_indices(':')
            .map(|(i, _)| (&raw[..i], &raw[i + 1..]))
            .find(|(b, l)| !b.ends_with('\\') && l.starts_with(|c: char| c.is_ascii_digit()));
        let (body, line) = match split {
            Some((b, l)) => (b, l.split(':').next()),
            None => (raw, None),
        };
        let has_sep = body.contains(['/', '\\']);
        let ext = body.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !has_sep && !KNOWN_EXT.contains(&ext.as_str()) {
            return None;
        }
        // Dotted words that are not files (e.g. versions) never reach here:
        // the extension starts with a letter.
        let parts: Vec<&str> = body
            .split(['/', '\\'])
            .filter(|p| !p.is_empty() && *p != "~" && *p != "." && *p != ".." && !p.ends_with(':'))
            .collect();
        let tail = path_tail(&parts, detail);
        let mut w = tail
            .iter()
            .map(|p| path_part(p))
            .collect::<Vec<_>>()
            .join(" slash ");
        if let Some(l) = line {
            w.push_str(&format!(", line {}", cardinal_str(l)));
        }
        Some(w)
    })
}

fn version_parts(v: &str) -> String {
    v.split('.')
        .map(cardinal_str)
        .collect::<Vec<_>>()
        .join(" point ")
}

/// A pre-release tag: "-beta.1" "beta one".
fn pre_release(tag: &str) -> String {
    let tag = tag.trim_start_matches('-');
    if tag.is_empty() {
        return String::new();
    }
    let words = tag
        .split('.')
        .map(|p| {
            if p.chars().all(|c| c.is_ascii_digit()) {
                cardinal_str(p)
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(" {words}")
}

fn versions(t: &str) -> String {
    let r = &*RULES;
    let named = |c: &Captures| {
        format!(
            "{} {}{}",
            &c[1],
            version_parts(&c[2]),
            pre_release(c.get(3).map_or("", |m| m.as_str()))
        )
    };
    let mut t = each(t, &r.version_word, |c, s, e| {
        free(t, s, e).then(|| named(c))
    });
    let cur = t.clone();
    t = each(&cur, &r.version_tool, |c, s, e| {
        free(&cur, s, e).then(|| named(c))
    });
    let cur = t.clone();
    t = each(&cur, &r.version_v, |c, s, e| {
        // `V8`, `V2 rockets`: a capital V and a whole number is a name.
        if c[0].starts_with('V') && !c[1].contains('.') {
            return None;
        }
        // "Version v1.20" says "version" once (2026-10-14).
        let said = cur[..s]
            .trim_end()
            .rsplit(|ch: char| !ch.is_alphanumeric())
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("version"));
        let lead = if said { "" } else { "version " };
        free_left(&cur, s, e).then(|| {
            format!(
                "{lead}{}{}",
                version_parts(&c[1]),
                pre_release(c.get(2).map_or("", |m| m.as_str()))
            )
        })
    });
    let cur = t.clone();
    each(&cur, &r.version_bare, |c, s, e| {
        // `12.12.2026` is a date, not a version.
        let p: Vec<&str> = c[1].split('.').collect();
        if p.len() == 3 && p[2].len() == 4 && p[0].len() <= 2 && p[1].len() <= 2 {
            return None;
        }
        // `section 3.2.1` numbers a part: no "version".
        let label = cur[..s]
            .trim_end()
            .rsplit(|ch: char| !ch.is_alphanumeric())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let lead = if matches!(
            label.as_str(),
            "section"
                | "sections"
                | "step"
                | "steps"
                | "clause"
                | "chapter"
                | "part"
                | "paragraph"
                | "figure"
                | "fig"
                | "table"
                | "item"
                | "rule"
                | "appendix"
                | "article"
                | "see"
                | "listing"
                | "example"
                | "exercise"
                | "task"
                | "point"
        ) {
            ""
        } else {
            "version "
        };
        free_left(&cur, s, e).then(|| {
            format!(
                "{lead}{}{}",
                version_parts(&c[1]),
                pre_release(c.get(2).map_or("", |m| m.as_str()))
            )
        })
    })
}

fn phones(t: &str) -> String {
    let r = &*RULES;
    let groups = |g: &str| {
        g.split([' ', '-'])
            .filter(|p| !p.is_empty())
            .map(spell_digits)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let t = each(t, &r.phone_plus, |c, s, e| {
        free(t, s, e).then(|| format!("plus {}, {}", spell_digits(&c[1]), groups(&c[2])))
    });
    let cur = t.clone();
    each(&cur, &r.phone_cue, |c, s, e| {
        free(&cur, s, e).then(|| format!("{} {}", &c[1], groups(&c[2])))
    })
}

fn money(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.money, |c, s, e| {
        if matches!(prev(t, s), Some(p) if p.is_alphanumeric() || p == '\\' || p == '$')
            || wordy(next(t, e))
        {
            return None;
        }
        let (one, many, sub_one, sub_many) = match &c[1] {
            "$" => ("dollar", "dollars", "cent", "cents"),
            "£" => ("pound", "pounds", "penny", "pence"),
            _ => ("euro", "euros", "cent", "cents"),
        };
        let amount = c[2].replace(',', "");
        if let Some(to) = c.get(4) {
            return Some(format!(
                "{} to {} {many}",
                number_words("", &amount),
                number_words("", &to.as_str().replace(',', ""))
            ));
        }
        if let Some(scale) = c.get(3) {
            let scale = match scale.as_str() {
                "k" | "K" | "thousand" => "thousand",
                "m" | "M" | "million" => "million",
                "b" | "B" | "bn" | "billion" => "billion",
                _ => "trillion",
            };
            return Some(format!("{} {scale} {many}", number_words("", &amount)));
        }
        let (whole, cents) = match amount.split_once('.') {
            Some((w, f)) if f.len() <= 2 => (
                w.parse::<u64>().ok()?,
                format!("{f:0<2}").parse::<u64>().unwrap_or(0),
            ),
            Some(_) => return Some(format!("{} {many}", number_words("", &amount))),
            None => (amount.parse::<u64>().ok()?, 0),
        };
        let main = format!(
            "{} {}",
            cardinal(whole),
            if whole == 1 { one } else { many }
        );
        let sub = format!(
            "{} {}",
            cardinal(cents),
            if cents == 1 { sub_one } else { sub_many }
        );
        Some(match (whole, cents) {
            (0, c) if c > 0 => sub,
            (_, 0) => main,
            _ => format!("{main} and {sub}"),
        })
    });
    let kroner = |amount: &str, ore: Option<&str>| -> Option<String> {
        let n: u64 = amount.replace([' ', '.'], "").parse().ok()?;
        let ore = ore.filter(|o| *o != "-" && *o != "00");
        let unit = if n == 1 && ore.is_none() {
            "krone"
        } else {
            "kroner"
        };
        Some(match ore {
            Some(o) => format!("{} {unit} {}", cardinal(n), cardinal_str(o)),
            None => format!("{} {unit}", cardinal(n)),
        })
    };
    let cur = t.clone();
    let t = each(&cur, &r.nok_before, |c, s, e| {
        if !free(&cur, s, e) {
            return None;
        }
        kroner(&c[1], c.get(2).map(|m| m.as_str()))
    });
    let cur = t.clone();
    let t = each(&cur, &r.nok_after, |c, s, e| {
        if !free(&cur, s, e) {
            return None;
        }
        kroner(&c[1], c.get(2).map(|m| m.as_str()))
    });
    let name = |code: &str| match code {
        "USD" => "U S dollars",
        "EUR" => "euros",
        "GBP" => "pounds",
        "SEK" => "Swedish kronor",
        "DKK" => "Danish kroner",
        "JPY" => "yen",
        "CHF" => "Swiss francs",
        "CAD" => "Canadian dollars",
        "AUD" => "Australian dollars",
        "CNY" => "yuan",
        _ => "rupees",
    };
    let cur = t.clone();
    let t = each(&cur, &r.iso_money_after, |c, s, e| {
        // `3 CAD files`: a code before a noun names something else.
        let rest = &cur[e..];
        let noun = rest.strip_prefix(' ').is_some_and(|r| {
            let w: String = r.chars().take_while(|ch| ch.is_alphabetic()).collect();
            w.starts_with(char::is_lowercase)
                && !matches!(
                    w.as_str(),
                    "each" | "per" | "left" | "in" | "for" | "total" | "a" | "an"
                )
        });
        (free_left(&cur, s, e) && !noun)
            .then(|| format!("{} {}", number_words("", &c[1]), name(&c[2])))
    });
    let cur = t.clone();
    each(&cur, &r.iso_money_before, |c, s, e| {
        free(&cur, s, e).then(|| format!("{} {}", number_words("", &c[2]), name(&c[1])))
    })
}

fn measures(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.percent, |c, s, e| {
        if !free_left(t, s, e) && !(c[1].is_empty() && !wordy(prev(t, s))) {
            return None;
        }
        let sign = if sign_ok(t, s) { &c[1] } else { "" };
        Some(format!("{} percent", number_words(sign, &c[2])))
    });
    let cur = t.clone();
    let t = each(&cur, &r.measure, |c, s, e| {
        let unit = &c[5];
        let (_, one, many, spaced) = UNITS.iter().find(|u| u.0 == unit)?;
        // The unit ends the token ("5 kmh" is no unit); `s` and `l` need a
        // space or a decimal ("1990s" is a decade).
        if wordy(next(&cur, e)) || wordy(prev(&cur, s)) || matches!(prev(&cur, s), Some('.' | ','))
        {
            return None;
        }
        if *spaced && c[4].is_empty() && !c[3].contains('.') {
            return None;
        }
        // A lone capital or `h`/`m` after a space is a word, not a unit
        // ("2 V eight"): those units are written against the number.
        if matches!(unit, "V" | "W" | "h" | "m") && !c[4].is_empty() {
            return None;
        }
        let sign = if sign_ok(&cur, s) { &c[1] } else { "" };
        let value = number_words(sign, &c[3]);
        let range = c
            .get(2)
            .map(|a| format!("{} to ", number_words(sign, a.as_str())));
        let word = if c.get(2).is_none() && c[3] == *"1" {
            one
        } else {
            many
        };
        Some(match range {
            Some(from) => format!("{from}{} {word}", number_words("", &c[3])),
            None => format!("{value} {word}"),
        })
    });
    let cur = t.clone();
    let t = each(&cur, &r.times_x, |c, s, e| {
        free_left(&cur, s, e).then(|| format!("{} times", number_words("", &c[1])))
    });
    let cur = t.clone();
    each(&cur, &r.bits, |c, s, e| {
        free_left(&cur, s, e).then(|| format!("{} {}", cardinal_str(&c[1]), &c[2]))
    })
}

fn times(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.ampm, |c, s, e| {
        if !free_left(t, s, e) {
            return None;
        }
        let h: u64 = c[1].parse().ok()?;
        let m: u64 = c.get(2).map_or(Some(0), |m| m.as_str().parse().ok())?;
        if !(1..=12).contains(&h) || m > 59 {
            return None;
        }
        let when = match m {
            0 => cardinal(h),
            1..=9 => format!("{} oh {}", cardinal(h), cardinal(m)),
            _ => format!("{} {}", cardinal(h), cardinal(m)),
        };
        // A dot that also ends the sentence stays ("at 5 PM. Then").
        let after = t[e..].trim_start_matches(' ');
        let ends = !c[4].is_empty()
            && (e == t.len()
                || after.starts_with(['\n', '\r'])
                || (after.len() < t[e..].len() && after.starts_with(char::is_uppercase)));
        Some(format!(
            "{when} {} M{}",
            c[3].to_ascii_uppercase(),
            if ends { "." } else { "" }
        ))
    });
    let cur = t.clone();
    let t = each(&cur, &r.duration, |c, s, e| {
        if !free(&cur, s, e) {
            return None;
        }
        let (h, m, sec): (u64, u64, u64) =
            (c[2].parse().ok()?, c[3].parse().ok()?, c[4].parse().ok()?);
        if m > 59 || sec > 59 {
            return None;
        }
        let unit =
            |n: u64, one: &str| format!("{} {one}{}", cardinal(n), if n == 1 { "" } else { "s" });
        let mut parts = Vec::new();
        if h > 0 {
            parts.push(unit(h, "hour"));
        }
        if m > 0 {
            parts.push(unit(m, "minute"));
        }
        if sec > 0 || parts.is_empty() {
            parts.push(unit(sec, "second"));
        }
        let words = match parts.len() {
            1 => parts[0].clone(),
            n => format!("{} and {}", parts[..n - 1].join(" "), parts[n - 1]),
        };
        Some(format!("{} {words}", &c[1]))
    });
    let cur = t.clone();
    let t = each(&cur, &r.clock, |c, s, e| {
        if !free_left(&cur, s, e) {
            return None;
        }
        let (h, m): (u64, u64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        if h > 23 || m > 59 {
            return None;
        }
        let mut w = clock(h, m);
        if let Some(sec) = c.get(3) {
            let sec: u64 = sec.as_str().parse().ok()?;
            if sec > 59 {
                return None;
            }
            if sec > 0 {
                w.push_str(&format!(" and {} seconds", cardinal(sec)));
            }
        }
        Some(w)
    });
    let cur = t.clone();
    each(&cur, &r.ratio, |c, s, e| {
        free_left(&cur, s, e).then(|| format!("{} to {}", cardinal_str(&c[1]), cardinal_str(&c[2])))
    })
}

fn dates(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.date_num, |c, s, e| {
        if c[2] != c[4] || !free_left(t, s, e) {
            return None;
        }
        let (a, b, y): (u64, u64, u64) =
            (c[1].parse().ok()?, c[3].parse().ok()?, c[5].parse().ok()?);
        // Day first, unless only month first makes a date (12/31/2026).
        let (d, m) = if b > 12 && a <= 12 { (b, a) } else { (a, b) };
        date_words(d, (m as usize).checked_sub(1)?, Some(y))
    });
    let cur = t.clone();
    let t = each(&cur, &r.date_short, |c, s, e| {
        if !free(&cur, s, e) {
            return None;
        }
        let (d, m): (u64, u64) = (c[2].parse().ok()?, c[3].parse().ok()?);
        Some(format!(
            "{} {}",
            &c[1],
            date_words(d, (m as usize).checked_sub(1)?, None)?
        ))
    });
    let cur = t.clone();
    let t = each(&cur, &r.date_dm, |c, s, e| {
        if !free_left(&cur, s, e) {
            return None;
        }
        let d: u64 = c[1].parse().ok()?;
        let y = c.get(3).and_then(|m| m.as_str().parse().ok());
        date_words(d, month_of(&c[2])?, y)
    });
    let cur = t.clone();
    each(&cur, &r.date_md, |c, s, e| {
        if !free(&cur, s, e) {
            return None;
        }
        let d: u64 = c[2].parse().ok()?;
        // "May I", "March on" have no number: no match. "Mar 5" is a date.
        let y = c.get(3).and_then(|m| m.as_str().parse().ok());
        date_words(d, month_of(&c[1])?, y)
    })
}

fn ranges(t: &str) -> String {
    let r = &*RULES;
    each(t, &r.range, |c, s, e| {
        if !free_left(t, s, e) || matches!(prev(t, s), Some('-' | '+')) {
            return None;
        }
        let (a, b) = (&c[1], &c[2]);
        let (x, y): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
        let years = a.len() == 4 && (1000..=2099).contains(&x);
        if years && b.len() == 2 {
            let full = x / 100 * 100 + y;
            return (full > x).then(|| format!("{} to {}", year(x), cardinal(y)));
        }
        if x >= y
            || !(a.len() == b.len() || b.len() <= 3)
            || a.starts_with('0')
            || b.starts_with('0')
        {
            return None;
        }
        if years && b.len() == 4 {
            return Some(format!("{} to {}", year(x), year(y)));
        }
        Some(format!("{} to {}", cardinal(x), cardinal(y)))
    })
}

/// "one half", "three quarters", "three eighths".
fn fraction_words(n: u64, d: u64) -> Option<String> {
    if d < 2 || n == 0 {
        return None;
    }
    let den = match d {
        2 => "half".to_string(),
        4 => "quarter".to_string(),
        _ => ordinal(d),
    };
    let den = if n == 1 {
        den
    } else if d == 2 {
        "halves".into()
    } else {
        format!("{den}s")
    };
    Some(format!("{} {den}", cardinal(n)))
}

fn fractions(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.mixed_fraction, |c, s, e| {
        if !free_left(t, s, e) {
            return None;
        }
        let (w, n, d): (u64, u64, u64) =
            (c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?);
        if n >= d {
            return None;
        }
        let part = fraction_words(n, d)?;
        let part = if n == 1 {
            format!("a {}", part.trim_start_matches("one "))
        } else {
            part
        };
        Some(format!("{} and {part}", cardinal(w)))
    });
    let cur = t.clone();
    let t = each(&cur, &r.rating, |c, s, e| {
        let d: u64 = c[3].parse().ok()?;
        (free_left(&cur, s, e) && !matches!(next(&cur, e), Some('/')))
            .then(|| format!("{} out of {}", decimal(&c[1], &c[2]), cardinal(d)))
    });
    let cur = t.clone();
    let t = each(&cur, &r.fraction, |c, s, e| {
        if !free_left(&cur, s, e) || matches!(next(&cur, e), Some('/')) {
            return None;
        }
        let (n, d): (u64, u64) = (c[1].parse().ok()?, c[2].parse().ok()?);
        if (n, d) == (24, 7) {
            return Some("twenty four seven".into());
        }
        // A count (`12/12 tests`, `step 1/3`, `45/50`), not a fraction.
        let before = cur[..s]
            .trim_end()
            .rsplit(|ch: char| !ch.is_alphabetic())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let counter = matches!(
            before.as_str(),
            "step"
                | "page"
                | "test"
                | "tests"
                | "part"
                | "round"
                | "score"
                | "passed"
                | "failed"
                | "item"
                | "task"
                | "phase"
                | "stage"
                | "file"
                | "files"
                | "question"
                | "slide"
                | "level"
        );
        if n == 0 || n >= d || d > 16 || counter {
            return Some(format!("{} of {}", cardinal(n), cardinal(d)));
        }
        fraction_words(n, d)
    });
    let mut t = t
        .replace('½', "one half")
        .replace('¼', "one quarter")
        .replace('¾', "three quarters")
        .replace('⅓', "one third")
        .replace('⅔', "two thirds")
        .replace('⅛', "one eighth");
    for (pat, to) in SLASH_WORDS.iter() {
        if !t.contains('/') {
            break;
        }
        let cur = t.clone();
        t = each(&cur, pat, |_, s, e| {
            (!wordy(prev(&cur, s)) && !wordy(next(&cur, e))).then(|| to.to_string())
        });
    }
    t
}

fn numbers(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.ordinal, |c, s, e| {
        free_left(t, s, e)
            .then(|| c[1].parse().ok().map(ordinal))
            .flatten()
    });
    let cur = t.clone();
    // `1 000 000` (space-grouped thousands) as one number.
    let t = each(&cur, &r.grouped_space, |c, s, e| {
        if !free_left(&cur, s, e) || matches!(next(&cur, e), Some('.' | ',')) {
            return None;
        }
        let sign = if sign_ok(&cur, s) { &c[1] } else { "" };
        Some(number_words(sign, &c[2].replace(' ', "")))
    });
    let cur = t.clone();
    let t = each(&cur, &r.grouped, |c, s, e| {
        if !free_left(&cur, s, e) && !(c[1].is_empty() && !wordy(prev(&cur, s))) {
            return None;
        }
        let sign = if sign_ok(&cur, s) { &c[1] } else { "" };
        Some(number_words(sign, &c[2]))
    });
    let cur = t.clone();
    let t = each(&cur, &r.decimal, |c, s, e| {
        let at = if c[1].is_empty() { s } else { s + 1 };
        if wordy(prev(&cur, at)) && !c[1].is_empty() {
            return None;
        }
        if wordy(prev(&cur, s))
            || matches!(prev(&cur, s), Some('.'))
            || wordy(next(&cur, e))
            || matches!(next(&cur, e), Some('.'))
        {
            return None;
        }
        let sign = if sign_ok(&cur, s) { &c[1] } else { return None };
        let mut w = decimal(&c[2], &c[3]);
        if !sign.is_empty() {
            w = format!("minus {w}");
        }
        Some(w)
    });
    let cur = t.clone();
    let t = each(&cur, &r.decade, |c, s, e| {
        if !free_left(&cur, s, e) {
            return None;
        }
        let d = &c[1];
        let n: u64 = d.parse().ok()?;
        // Two digits are a decade only as `'80s` or after "the", "early"
        // and the like; else seconds (`30s`).
        let ctx = cur[..s]
            .trim_end()
            .rsplit(|ch: char| !ch.is_alphabetic())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let decade = c[0].starts_with('\'')
            || matches!(
                ctx.as_str(),
                "the" | "early" | "mid" | "late" | "his" | "her" | "their" | "my" | "your" | "our"
            );
        if d.len() == 2 && !decade {
            return (!c[0].contains('\'')).then(|| format!("{} seconds", cardinal(n)));
        }
        if !n.is_multiple_of(10) {
            return None;
        }
        Some(match d.len() {
            2 => plural(&cardinal(n)),
            4 => plural(&year(n)),
            _ => return None,
        })
    });
    let cur = t.clone();
    let t = each(&cur, &r.negative, |c, s, e| {
        (sign_ok(&cur, s) && !wordy(next(&cur, e)) && !matches!(next(&cur, e), Some('.' | ',')))
            .then(|| format!("minus {}", cardinal_str(&c[1])))
    });
    let cur = t.clone();
    let t = each(&cur, &r.leading_zero, |c, s, e| {
        free_left(&cur, s, e).then(|| spell_digits(&c[0]))
    });
    let cur = t.clone();
    each(&cur, &r.big, |c, s, e| {
        if !free_left(&cur, s, e) {
            return None;
        }
        let n: u64 = c[1].parse().ok()?;
        let before = cur[..s].trim_end();
        let cue = before
            .rsplit(|ch: char| !ch.is_alphabetic())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_year = c[1].len() == 4
            && (1000..=2099).contains(&n)
            && matches!(
                cue.as_str(),
                "in" | "since"
                    | "by"
                    | "until"
                    | "from"
                    | "during"
                    | "before"
                    | "after"
                    | "of"
                    | "year"
                    | "circa"
                    | "around"
                    | "till"
                    | "through"
                    | "january"
                    | "february"
                    | "march"
                    | "april"
                    | "may"
                    | "june"
                    | "july"
                    | "august"
                    | "september"
                    | "october"
                    | "november"
                    | "december"
                    | "jan"
                    | "feb"
                    | "mar"
                    | "apr"
                    | "jun"
                    | "jul"
                    | "aug"
                    | "sep"
                    | "sept"
                    | "oct"
                    | "nov"
                    | "dec"
            );
        Some(if is_year { year(n) } else { cardinal(n) })
    })
}

fn abbreviations(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.abbrev, |c, s, e| {
        if wordy(prev(t, s)) {
            return None;
        }
        let ends = matches!(next(t, e), None | Some('\n'));
        Some(match &c[0] {
            "e.g." => "for example".into(),
            "i.e." => "that is".into(),
            "etc." => {
                if ends
                    || matches!(next(t, e), Some(' '))
                        && t[e..].trim_start().starts_with(char::is_uppercase)
                {
                    "et cetera.".into()
                } else {
                    "et cetera".into()
                }
            }
            "vs." => "versus".into(),
            "Dr." => "Doctor".into(),
            "Mr." => "Mister".into(),
            "Mrs." => "Missus".into(),
            "Ms." => "Mizz".into(),
            "approx." => "about".into(),
            other => {
                // `item no. 5`; a bare "no." ("Did it pass? no. 3 tests")
                // stays.
                let cue = t[..s]
                    .trim_end()
                    .rsplit(|ch: char| !ch.is_alphabetic())
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(
                    cue.as_str(),
                    "item"
                        | "issue"
                        | "page"
                        | "room"
                        | "step"
                        | "case"
                        | "rule"
                        | "version"
                        | "part"
                        | "section"
                        | "ticket"
                        | "bug"
                        | "order"
                        | "invoice"
                        | "account"
                        | "model"
                        | "serial"
                ) {
                    return None;
                }
                format!("number {}", &other[4..])
            }
        })
    });
    let cur = t.clone();
    each(&cur, &r.dotted_caps, |c, s, e| {
        (!wordy(prev(&cur, s))).then(|| {
            // A dot that also ends the sentence stays ("the U.S. Then").
            let rest = cur[e..].trim_start();
            let ends =
                cur[e..].starts_with(char::is_whitespace) && rest.starts_with(char::is_uppercase);
            format!("{}{}", spell(&c[0]), if ends { "." } else { "" })
        })
    })
}

fn symbols(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.hash_num, |c, s, _| {
        (!wordy(prev(t, s)) && !matches!(prev(t, s), Some('&' | '#')))
            .then(|| format!("number {}", cardinal_str(&c[1])))
    });
    let cur = t.clone();
    let t = each(&cur, &r.amp_letters, |c, s, e| {
        free(&cur, s, e).then(|| format!("{} and {}", &c[1], &c[2]))
    });
    let cur = t.clone();
    let t = each(&cur, &r.call, |c, s, _| {
        (!wordy(prev(&cur, s))).then(|| c[1].to_string())
    });
    let cur = t.clone();
    let t = each(&cur, &r.flag, |c, s, e| {
        let start_ok = s == 0 || prev(&cur, s).is_some_and(char::is_whitespace);
        (start_ok && !wordy(next(&cur, e))).then(|| {
            let dash = if c[1].len() == 2 { "dash dash" } else { "dash" };
            format!("{dash} {}", c[2].replace('-', " ").to_lowercase())
        })
    });
    t.replace('±', "plus or minus")
        .replace('×', "times")
        .replace('≥', "greater than or equal to")
        .replace('≤', "less than or equal to")
        .replace('≠', "not equal to")
        .replace('≈', "about")
        .replace('§', "section")
}

/// Identifiers, tech words, roman numerals after a keyword, letters with
/// digits, and capitalised words (acronyms).
fn words(t: &str) -> String {
    let r = &*RULES;
    let t = each(t, &r.roman, |c, s, e| {
        free(t, s, e)
            .then(|| roman(&c[2]).map(|n| format!("{} {}", &c[1], cardinal(n))))
            .flatten()
    });
    let shout = shouting(&t);
    let cur = t.clone();
    let t = each(&cur, &r.letters_digits, |c, s, e| {
        if wordy(prev(&cur, s)) || wordy(next(&cur, e)) || matches!(prev(&cur, s), Some('.')) {
            return None;
        }
        let letters = &c[1];
        if TECH.contains_key(&c[0]) {
            return None;
        }
        let head = if letters.chars().all(|ch| ch.is_ascii_uppercase()) {
            spell(letters)
        } else if letters.len() == 1 {
            letters.to_string()
        } else {
            return None;
        };
        Some(format!("{head} {}", cardinal_str(&c[2])))
    });
    let cur = t.clone();
    let t = each(&cur, &r.caps_plural, |c, s, e| {
        if wordy(prev(&cur, s)) || wordy(next(&cur, e)) || shout {
            return None;
        }
        let base = &c[1];
        if TECH.contains_key(&c[0]) || KEEP.contains(&c[0]) || c[0] == *"IDs" && false {
            return None;
        }
        let said = caps_word(base);
        Some(if said == spell(base) {
            format!("{said}'s")
        } else {
            format!("{said}s")
        })
    });
    let cur = t.clone();
    each(&cur, &r.word, |c, s, e| {
        let w = &c[0];
        if matches!(prev(&cur, s), Some('\u{E020}')) || wordy(prev(&cur, s)) || wordy(next(&cur, e))
        {
            return None;
        }
        if let Some(say) = TECH.get(w) {
            return Some((*say).to_string());
        }
        if BRANDS.contains(&w) {
            return None;
        }
        let snake = w.contains('_');
        let camel = w.chars().skip(1).any(char::is_uppercase) && w.chars().any(char::is_lowercase);
        if snake || camel {
            return Some(split_identifier(w, shout));
        }
        if w.chars().all(|ch| ch.is_ascii_uppercase()) && !shout {
            let said = caps_word(w);
            return (said != *w).then_some(said);
        }
        None
    })
}

/// Letters and digits as said: `MD5` "M D five", `x86` "x eighty six";
/// `None` when it is not that shape (or a known term).
fn letters_digits(p: &str) -> Option<String> {
    if TECH.contains_key(p) {
        return None;
    }
    let split = p.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = p.split_at(split);
    if letters.is_empty()
        || letters.len() > 5
        || !letters.chars().all(|c| c.is_ascii_alphabetic())
        || digits.is_empty()
        || digits.len() > 4
        || !digits.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let head = if letters.chars().all(|ch| ch.is_ascii_uppercase()) {
        spell(letters)
    } else if letters.len() == 1 {
        letters.to_string()
    } else {
        return None;
    };
    Some(format!("{head} {}", cardinal_str(digits)))
}

/// An identifier in prose as words (`getUserName` "get user name",
/// `HTTP_PORT` "H T T P port"); its parts read as words or terms.
fn split_identifier(w: &str, shout: bool) -> String {
    let screaming = !w.chars().any(char::is_lowercase);
    let split = code_speech::identifier(w);
    split
        .split(' ')
        .filter(|p| !p.is_empty())
        .map(|p| {
            if let Some(say) = TECH.get(p) {
                return (*say).to_string();
            }
            if let Some(said) = letters_digits(p) {
                return said;
            }
            let caps = p.chars().all(|c| c.is_ascii_uppercase()) && p.len() > 1;
            if caps && !shout {
                if screaming && (!spelled(p) || COMMON.contains(&p)) && !FORCED.contains(&p) {
                    return p.to_lowercase();
                }
                return caps_word(p);
            }
            if screaming {
                p.to_lowercase()
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's words are process-wide: one test at a time sets them.
    static WORDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_users_words_come_first_and_are_read_as_written() {
        let _one = WORDS.lock().unwrap_or_else(|e| e.into_inner());
        set_user_words(vec![
            ("Ove".into(), "Oo veh".into()),
            ("SQL".into(), "SQL server".into()),
            ("veh".into(), "vay".into()),
            ("Ola".into(), "Ola 2026".into()),
        ]);
        assert_eq!(
            pronounce("Hi Ove, the SQL query, Ola."),
            "Hi Oo veh, the SQL server query, Ola 2026."
        );
        // Inside a path or URL the word is left to those rules.
        set_user_words(vec![("Sonara".into(), "So nara".into())]);
        assert_eq!(
            pronounce(
                "See crates/Sonara/src/main.rs and https://github.com/Maxaubert/Sonara/pull/7"
            ),
            "See Sonara slash src slash main dot R S and github dot com, pull seven"
        );
        assert_eq!(pronounce("I like Sonara."), "I like So nara.");
        // A digit as a word never reaches a placeholder.
        set_user_words(vec![("0".into(), "zero".into())]);
        assert_eq!(pronounce("Run `a` now"), "Run `a` now");
        set_user_words(Vec::new());
        assert_eq!(pronounce("the SQL query"), "the sequel query");
    }

    #[test]
    fn the_path_setting_reads_as_much_of_a_path_as_asked() {
        let p = r"See C:\Users\Admin\AppData\Local\Sonara\logs\sonarad.log now.";
        let said = |d| paths_with(p, d);
        assert_eq!(
            said(PathDetail::Short),
            "See Sonara slash logs slash sonarad dot log now."
        );
        assert_eq!(
            said(PathDetail::LastTwo),
            "See logs slash sonarad dot log now."
        );
        assert_eq!(
            said(PathDetail::LastThree),
            "See Sonara slash logs slash sonarad dot log now."
        );
        assert_eq!(
            said(PathDetail::Full),
            "See Users slash Admin slash AppData slash Local slash Sonara slash logs slash \
             sonarad dot log now."
        );
        // Short keeps a file in a home folder and the last three of a long one.
        assert_eq!(
            paths_with(r"C:\Users\Admin\notes.txt", PathDetail::Short),
            "notes dot T X T"
        );
        assert_eq!(
            paths_with("/home/ove/a/b/c/d.rs", PathDetail::Short),
            "b slash c slash d dot R S"
        );
        for d in PathDetail::NAMES {
            assert_eq!(PathDetail::from_name(d).map(PathDetail::name), Some(*d));
        }
    }

    #[test]
    fn placeholders_never_leak_or_collide() {
        let many: String = (0..1100).map(|i| format!("`c{i}` ")).collect();
        assert_eq!(pronounce(&many), many);
        assert_eq!(pronounce("a \u{E020}0\u{E021} and `x` b"), "a 0 and `x` b");
    }

    #[test]
    fn its_output_reads_the_same_again() {
        for s in [
            "Released v1.20 on 12.12.2026 at 14:30 for $5.99 (5 km, 50%).",
            "See crates/sonara-core/src/text.rs:42 and https://github.com/a/b/pull/7.",
            "The API uses JSON over HTTP; getUserName returns the ID.",
        ] {
            let once = pronounce(s);
            assert_eq!(pronounce(&once), once, "{s}");
        }
    }
}
