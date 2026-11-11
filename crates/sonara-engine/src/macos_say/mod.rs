//! The macOS system voice engine, on `/usr/bin/say`: zero download, licence
//! class `Os`, engine id `macos`. It speaks while Kokoro downloads, as
//! OneCore does on Windows. Each synthesis runs `say` once and reads the WAV
//! it writes (about 100 to 300 ms of overhead per sentence; an
//! AVSpeechSynthesizer engine is the planned follow-up).
//!
//! The pure parts (the voice list parse, the voice choice, the rate
//! mapping, the text guard, the failure text) live here so they are tested
//! on any machine; the process calls are in `say.rs`.
use crate::{EngineId, Error, MACOS_VOICES_FIX};

#[cfg(all(target_os = "macos", feature = "macos-say"))]
mod say;
#[cfg(all(target_os = "macos", feature = "macos-say"))]
pub use say::MacSay;

pub const ID: EngineId = EngineId("macos");

/// The program; a fixed path, never a PATH lookup.
pub const SAY: &str = "/usr/bin/say";

/// The sample format `say` writes: 16-bit little-endian PCM, mono, 22.05 kHz.
pub const DATA_FORMAT: &str = "LEI16@22050";

/// Sonara's normal rate (words per minute) is the voice's own normal speed.
pub const BASELINE_WPM: f64 = 200.0;
/// The `say -r` value of a voice's own normal speed. Measured 2026-11-11 on
/// macOS 27 (Apple Silicon): a 36-word sentence with no `-r` took as long as
/// with `-r 174` (Samantha), so `say` speaks at about 175 by default. Its
/// `-r` is linear in speed (150, 200 and 300 gave 213, 263 and 373 words a
/// minute by word count, a constant ratio), so Sonara's rate scales it.
pub const SAY_NORMAL_RATE: f64 = 175.0;
/// The same bounds as OneCore's SpeakingRate (0.5x to 6x normal).
pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 6.0;

/// Map a Sonara rate (words per minute, 200 = normal) to `say -r`, so 200
/// sounds as fast as `say` with no `-r`, and each engine reads a Sonara
/// rate at the same speed relative to its voice.
pub fn say_rate(wpm: u32) -> u32 {
    let speed = (wpm as f64 / BASELINE_WPM).clamp(MIN_SPEED, MAX_SPEED);
    (speed * SAY_NORMAL_RATE).round() as u32
}

/// How good a system voice sounds, from its name: macOS names the
/// downloadable higher-quality voices "<name> (Enhanced)" and
/// "<name> (Premium)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    Default,
    Enhanced,
    Premium,
}

/// A voice as `say -v '?'` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    /// The name `say -v` takes (also the voice id).
    pub name: String,
    /// BCP 47, e.g. `en-US` (`say` prints `en_US`).
    pub language: String,
    pub quality: Quality,
}

fn quality(name: &str) -> Quality {
    let lower = name.to_ascii_lowercase();
    if lower.contains("(premium)") {
        Quality::Premium
    } else if lower.contains("(enhanced)") {
        Quality::Enhanced
    } else {
        Quality::Default
    }
}

/// A locale as `say` prints it: `en_US`, `zh_CN`, `ar_001`, `en-scotland`.
fn is_locale(s: &str) -> bool {
    let mut parts = s.split(['_', '-']);
    let lang = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_lowercase())
        && !rest.is_empty()
        && rest
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Parse `say -v '?'`: one voice a line, "<name> <locale> # <sample>",
/// padded with spaces. A name may hold spaces and brackets ("Eddy (English
/// (US))", "Bad News"). Lines that do not parse are skipped, and a name
/// listed twice (macOS lists some voices once per installed variant) is
/// kept once.
pub fn parse_voices(listing: &str) -> Vec<VoiceInfo> {
    let mut out: Vec<VoiceInfo> = Vec::new();
    for line in listing.lines() {
        let head = match line.find(" #") {
            Some(i) => &line[..i],
            None => line,
        };
        let head = head.trim_end();
        let Some(split) = head.rfind(char::is_whitespace) else {
            continue;
        };
        let (name, locale) = (head[..split].trim(), head[split..].trim());
        if name.is_empty() || !is_locale(locale) {
            continue;
        }
        if out.iter().any(|v| v.name == name) {
            continue;
        }
        out.push(VoiceInfo {
            name: name.to_string(),
            language: locale.replace('_', "-"),
            quality: quality(name),
        });
    }
    out
}

/// Pick the voice for `wanted` (a voice name, any case; empty for the
/// default). The default is the best en-US Premium or Enhanced voice, then
/// any English one of those; with none installed, `None`: `say` then uses
/// the system voice the user picked in System Settings > Accessibility >
/// Spoken Content. No voice is chosen by name.
pub fn choose_voice<'a>(
    voices: &'a [VoiceInfo],
    wanted: &str,
) -> Result<Option<&'a VoiceInfo>, Error> {
    if !wanted.is_empty() {
        return voices
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(wanted))
            .map(Some)
            .ok_or_else(|| Error::UnknownVoice(wanted.to_string()));
    }
    let lang = |v: &VoiceInfo| v.language.to_ascii_lowercase();
    let best = |keep: &dyn Fn(&VoiceInfo) -> bool| {
        voices
            .iter()
            .filter(|v| v.quality > Quality::Default && keep(v))
            // The first listed among the best quality.
            .fold(None::<&VoiceInfo>, |best, v| match best {
                Some(b) if b.quality >= v.quality => Some(b),
                _ => Some(v),
            })
    };
    Ok(best(&|v| lang(v) == "en-us").or_else(|| best(&|v| lang(v).starts_with("en"))))
}

/// `say` reads "[[" ... "]]" as an embedded command (rate, volume, voice
/// changes), which text from an agent must never trigger: break each "[["
/// with a space, which is not spoken.
pub fn guard_text(text: &str) -> String {
    // Split every run of '[': a single pass of replace("[[", "[ [") leaves
    // "[[" in "[[[[", which `say` would read as a command.
    let mut out = String::with_capacity(text.len() + 8);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '[' && prev == '[' {
            out.push(' ');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Turn a failed `say` into an `Error`: its exit status and the first line
/// of what it printed (e.g. "Speaking failed: -241" for a voice it cannot
/// load; the engine checks names against the list before it runs `say`).
pub fn classify_failure(status: &str, stderr: &str) -> Error {
    match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(l) => Error::Engine(format!("macOS speech failed ({status}): {l}")),
        None => Error::Engine(format!("macOS speech failed ({status})")),
    }
}

/// The error when `say` lists no voice at all.
pub fn no_voices() -> Error {
    Error::Engine(format!(
        "no usable macOS voices: `say` lists none. {MACOS_VOICES_FIX}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as `say -v '?'` prints them on macOS 27 (2026-11-11), plus the
    /// quality suffixes of downloaded voices.
    const LISTING: &str = "\
Albert              en_US    # Hello! My name is Albert.
Alice               it_IT    # Ciao! Mi chiamo Alice.
Amélie              fr_CA    # Bonjour! Je m\u{2019}appelle Amélie.
Bad News            en_US    # Hello! My name is Bad News.
Carmit              he_IL    # \u{5e9}\u{5dc}\u{5d5}\u{5dd}
Daniel              en_GB    # Hello! My name is Daniel.
Eddy (German (Germany)) de_DE    # Hallo! Ich heiße Eddy.
Eddy (English (US)) en_US    # Hello! My name is Eddy.
Samantha (English (US)) en_US    # Hello! My name is Samantha.
Samantha (English (US)) en_US    # Hello! My name is Samantha.
Majed               ar_001   # Hello! My name is Majed.
Fiona (Enhanced)    en-scotland # Hello! My name is Fiona.
Ava (Premium)       en_US    # Hello! My name is Ava.
Zoe (Enhanced)      en_US    # Hello! My name is Zoe.

not a voice line
";

    #[test]
    fn the_voice_list_keeps_names_with_spaces_and_brackets() {
        let voices = parse_voices(LISTING);
        let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Albert",
                "Alice",
                "Amélie",
                "Bad News",
                "Carmit",
                "Daniel",
                "Eddy (German (Germany))",
                "Eddy (English (US))",
                "Samantha (English (US))",
                "Majed",
                "Fiona (Enhanced)",
                "Ava (Premium)",
                "Zoe (Enhanced)",
            ]
        );
    }

    #[test]
    fn the_voice_list_turns_locales_into_bcp47_and_reads_quality() {
        let voices = parse_voices(LISTING);
        let find = |n: &str| voices.iter().find(|v| v.name == n).unwrap();
        assert_eq!(find("Albert").language, "en-US");
        assert_eq!(find("Majed").language, "ar-001");
        assert_eq!(find("Fiona (Enhanced)").language, "en-scotland");
        assert_eq!(find("Albert").quality, Quality::Default);
        assert_eq!(find("Zoe (Enhanced)").quality, Quality::Enhanced);
        assert_eq!(find("Ava (Premium)").quality, Quality::Premium);
    }

    #[test]
    fn an_empty_or_garbled_listing_lists_no_voice() {
        assert!(parse_voices("").is_empty());
        assert!(parse_voices("say: no voices\n# comment\nName only\n").is_empty());
    }

    #[test]
    fn the_default_voice_is_the_best_en_us_premium_or_enhanced_one() {
        let voices = parse_voices(LISTING);
        let pick = |v: &[VoiceInfo]| {
            choose_voice(v, "")
                .unwrap()
                .map(|v| v.name.clone())
                .unwrap_or_default()
        };
        assert_eq!(pick(&voices), "Ava (Premium)");
        let no_premium: Vec<_> = voices
            .iter()
            .filter(|v| v.quality != Quality::Premium)
            .cloned()
            .collect();
        assert_eq!(pick(&no_premium), "Zoe (Enhanced)");
        // No en-US one: another English Premium or Enhanced voice.
        let not_us: Vec<_> = voices
            .iter()
            .filter(|v| v.language != "en-US")
            .cloned()
            .collect();
        assert_eq!(pick(&not_us), "Fiona (Enhanced)");
    }

    #[test]
    fn without_a_premium_or_enhanced_voice_say_uses_the_system_voice() {
        let plain: Vec<_> = parse_voices(LISTING)
            .into_iter()
            .filter(|v| v.quality == Quality::Default)
            .collect();
        assert!(!plain.is_empty());
        assert_eq!(choose_voice(&plain, "").unwrap(), None);
        assert_eq!(choose_voice(&[], "").unwrap(), None);
    }

    #[test]
    fn a_named_voice_matches_its_name_in_any_case() {
        let voices = parse_voices(LISTING);
        assert_eq!(
            choose_voice(&voices, "eddy (english (us))")
                .unwrap()
                .map(|v| v.name.as_str()),
            Some("Eddy (English (US))")
        );
        assert_eq!(
            choose_voice(&voices, "Nobody"),
            Err(Error::UnknownVoice("Nobody".into()))
        );
    }

    #[test]
    fn rate_mapping_makes_200_the_voice_normal_speed() {
        let table = [
            (0, 88),
            (100, 88),
            (150, 131),
            (200, 175),
            (250, 219),
            (300, 263),
            (400, 350),
            (1200, 1050),
            (u32::MAX, 1050),
        ];
        for (wpm, expected) in table {
            assert_eq!(say_rate(wpm), expected, "wpm {wpm}");
        }
    }

    #[test]
    fn embedded_say_commands_in_the_text_are_not_obeyed() {
        assert_eq!(guard_text("a [[rate 50]] b"), "a [ [rate 50]] b");
        assert_eq!(guard_text("[[[["), "[ [ [ [");
        for n in 2..9 {
            assert!(!guard_text(&"[".repeat(n)).contains("[["), "{n} brackets");
        }
        assert_eq!(guard_text("plain [x]"), "plain [x]");
    }

    #[test]
    fn a_failure_keeps_say_message_and_no_voices_names_the_repair() {
        assert_eq!(
            classify_failure("exit status: 1", "Speaking failed: -241\n"),
            Error::Engine("macOS speech failed (exit status: 1): Speaking failed: -241".into())
        );
        assert_eq!(
            classify_failure("exit status: 1", "\n  Opening output file failed: -43\n"),
            Error::Engine(
                "macOS speech failed (exit status: 1): Opening output file failed: -43".into()
            )
        );
        assert_eq!(
            classify_failure("signal: 9", ""),
            Error::Engine("macOS speech failed (signal: 9)".into())
        );
        let none = no_voices().to_string();
        assert!(
            none.contains("System Settings > Accessibility > Spoken Content"),
            "{none}"
        );
    }
}
