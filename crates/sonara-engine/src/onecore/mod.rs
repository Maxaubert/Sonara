//! The Windows OneCore engine (`Windows.Media.SpeechSynthesis`): zero
//! download, licence class `Os`. The pure parts (rate mapping, voice choice,
//! error classification) live here so they are tested on any machine; the
//! WinRT calls are in `winrt.rs`.
use crate::{EngineId, Error};

#[cfg(all(windows, feature = "onecore"))]
mod winrt;
#[cfg(all(windows, feature = "onecore"))]
pub use winrt::OneCore;

pub const ID: EngineId = EngineId("onecore");

/// Sonara's default rate (words per minute) maps to SpeakingRate 1.0.
pub const BASELINE_WPM: f64 = 200.0;
/// WinRT rejects a SpeakingRate outside this range.
pub const MIN_SPEAKING_RATE: f64 = 0.5;
pub const MAX_SPEAKING_RATE: f64 = 6.0;

/// Map a Sonara rate (words per minute, 200 = normal) to the SpeakingRate
/// multiplier, clamped to what WinRT accepts. Same rule as the Python
/// `wpm_to_speaking_rate`.
pub fn speaking_rate(wpm: u32) -> f64 {
    (wpm as f64 / BASELINE_WPM).clamp(MIN_SPEAKING_RATE, MAX_SPEAKING_RATE)
}

/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) and (ERROR_PATH_NOT_FOUND).
const FILE_NOT_FOUND: i32 = 0x8007_0002_u32 as i32;
const PATH_NOT_FOUND: i32 = 0x8007_0003_u32 as i32;

/// Turn a failed synthesis into an `Error`. "File not found" while voices are
/// listed means their data files are missing (D7), which gets its own error
/// with the repair; everything else keeps the platform message.
pub fn classify_failure(hresult: i32, message: &str, voice_names: &[String]) -> Error {
    if hresult == FILE_NOT_FOUND || hresult == PATH_NOT_FOUND {
        if voice_names.is_empty() {
            return Error::NoVoices;
        }
        let verb = if voice_names.len() == 1 { "is" } else { "are" };
        return Error::MissingVoiceData {
            voices: format!("{} {}", voice_names.join(", "), verb),
        };
    }
    let message = message.trim();
    if message.is_empty() {
        Error::Engine(format!("OneCore synthesis failed ({hresult:#010x})"))
    } else {
        Error::Engine(format!(
            "OneCore synthesis failed ({hresult:#010x}): {message}"
        ))
    }
}

/// A voice as OneCore lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// Pick the voice for `wanted` (a voice id or display name, any case; empty
/// for the default). The default is the first en-US OneCore voice, then any
/// en-US voice, then the system default (`None`), as the Python backend.
pub fn choose_voice<'a>(
    voices: &'a [VoiceInfo],
    wanted: &str,
) -> Result<Option<&'a VoiceInfo>, Error> {
    if !wanted.is_empty() {
        return voices
            .iter()
            .find(|v| v.id.eq_ignore_ascii_case(wanted) || v.name.eq_ignore_ascii_case(wanted))
            .map(Some)
            .ok_or_else(|| Error::UnknownVoice(wanted.to_string()));
    }
    let en_us = |v: &&VoiceInfo| v.language.to_ascii_lowercase().starts_with("en-us");
    let onecore = |v: &&VoiceInfo| v.id.to_ascii_lowercase().contains("speech_onecore");
    Ok(voices
        .iter()
        .filter(en_us)
        .find(onecore)
        .or_else(|| voices.iter().find(en_us)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_mapping_table() {
        let table = [
            (0, 0.5),
            (50, 0.5),
            (100, 0.5),
            (150, 0.75),
            (200, 1.0),
            (250, 1.25),
            (300, 1.5),
            (400, 2.0),
            (1200, 6.0),
            (5000, 6.0),
            (u32::MAX, 6.0),
        ];
        for (wpm, expected) in table {
            assert_eq!(speaking_rate(wpm), expected, "wpm {wpm}");
        }
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn file_not_found_with_listed_voices_is_missing_voice_data() {
        let voices = names(&["Microsoft David", "Microsoft Zira", "Microsoft Mark"]);
        let e = classify_failure(FILE_NOT_FOUND, "The system cannot find the file", &voices);
        assert_eq!(
            e,
            Error::MissingVoiceData {
                voices: "Microsoft David, Microsoft Zira, Microsoft Mark are".into()
            }
        );
        let text = e.to_string();
        assert!(
            text.starts_with(
                "Microsoft David, Microsoft Zira, Microsoft Mark are listed but cannot speak: \
                 their voice data is missing from this PC (synthesis fails with 'file not found')."
            ),
            "{text}"
        );
        assert!(text.contains("DISM /Online /Add-Capability"), "{text}");
        let one = classify_failure(PATH_NOT_FOUND, "", &names(&["Microsoft Zira"]));
        assert!(one.to_string().starts_with("Microsoft Zira is listed"));
    }

    #[test]
    fn file_not_found_without_voices_is_no_voices() {
        assert_eq!(classify_failure(FILE_NOT_FOUND, "", &[]), Error::NoVoices);
        let text = Error::NoVoices.to_string();
        assert!(
            text.starts_with(
                "no usable Windows voices: none are installed, or their voice data is missing \
                 from this PC."
            ),
            "{text}"
        );
        assert!(text.contains("DISM /Online /Add-Capability"), "{text}");
    }

    #[test]
    fn other_failures_keep_the_platform_message() {
        let e = classify_failure(0x8000_4005_u32 as i32, " Unspecified error ", &[]);
        assert_eq!(
            e,
            Error::Engine("OneCore synthesis failed (0x80004005): Unspecified error".into())
        );
        let bare = classify_failure(0x8000_4005_u32 as i32, "", &[]);
        assert_eq!(
            bare,
            Error::Engine("OneCore synthesis failed (0x80004005)".into())
        );
    }

    fn v(id: &str, name: &str, language: &str) -> VoiceInfo {
        VoiceInfo {
            id: id.into(),
            name: name.into(),
            language: language.into(),
        }
    }

    #[test]
    fn default_voice_prefers_en_us_onecore() {
        let voices = vec![
            v("HKLM\\Speech\\Voices\\Hedda", "Microsoft Hedda", "de-DE"),
            v(
                "HKLM\\Speech\\Voices\\David",
                "Microsoft David Desktop",
                "en-US",
            ),
            v(
                "HKLM\\Speech_OneCore\\Voices\\Zira",
                "Microsoft Zira",
                "en-US",
            ),
        ];
        assert_eq!(
            choose_voice(&voices, "").unwrap().map(|v| v.name.as_str()),
            Some("Microsoft Zira")
        );
        assert_eq!(
            choose_voice(&voices[..2], "")
                .unwrap()
                .map(|v| v.name.as_str()),
            Some("Microsoft David Desktop")
        );
        assert_eq!(choose_voice(&voices[..1], "").unwrap(), None);
    }

    #[test]
    fn a_named_voice_matches_id_or_name_in_any_case() {
        let voices = vec![v("ID-ZIRA", "Microsoft Zira", "en-US")];
        assert_eq!(
            choose_voice(&voices, "microsoft zira").unwrap(),
            Some(&voices[0])
        );
        assert_eq!(choose_voice(&voices, "id-zira").unwrap(), Some(&voices[0]));
        assert_eq!(
            choose_voice(&voices, "Microsoft Hazel"),
            Err(Error::UnknownVoice("Microsoft Hazel".into()))
        );
    }
}
