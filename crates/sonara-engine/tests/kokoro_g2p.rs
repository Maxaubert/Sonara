//! Golden cases for Kokoro's text rules and G2P: the M0 corpus lines plus
//! numbers, versions, paths and identifiers. A change to the rules, the
//! custom lexicon or the vendored misaki shows up here as a diff.
//!
//! Regenerate after an intended change, then review the diff:
//! `SONARA_BLESS=1 cargo test -p sonara-engine --test kokoro_g2p`
#![cfg(feature = "kokoro")]
use serde_json::{json, Value};
use sonara_engine::kokoro::phonemes::{self, Phonemizer};
use sonara_engine::kokoro::text;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/kokoro_g2p.json");

/// The inputs. The first ten are the M0 corpus's short lines (after
/// Sonara's `normalize_for_speech`, as the spike fed them).
const CASES: &[&str] = &[
    "Call get user id before you open the SessionChannel, otherwise the router drops the event.",
    "I changed src/sonara/daemon/ingest.py and tests/test_router.py, then reran the suite.",
    "CI is green on the PR, and the API now returns JSON instead of XML.",
    "Pi is roughly 3.14, and the plugin is now at v0.8.5 after 1689 tests passed.",
    "The docs live at link if you want the details.",
    "1: Run sonara keymap --reset, then press Ctrl+Alt+M once to check the mute cycle.",
    "The KokoroEngine calls normalize for speech, and the WASAPI output thread uses cpal.",
    "Set SONARA_HOME to %LOCALAPPDATA%\\Sonara and restart sonarad.exe on port 8765.",
    "Use npm i @sonara/client, then call connect() and onState() from the Electron main process.",
    "The ort 2.0 rc crate wraps onnxruntime.dll, and cargo-deny bans espeak-ng and piper-phonemize.",
    // Numbers and versions.
    "It took 42 seconds, 1,024 bytes and 3 retries.",
    "Python 3.14 replaced 3.12; Sonara 0.10.0 ships today.",
    "Version v1.28.2 of ONNX Runtime, on the 21st of May.",
    // Paths and files.
    "Open C:\\Users\\me\\AppData\\Local\\Sonara\\models\\kokoro\\v1.0.",
    "Edit Cargo.toml, README.md and settings.json.",
    // Identifiers and acronyms.
    "The GUIDs, IDs and URLs come from the HTTPServer.",
    "Call fetchModelFiles() then useState in the React component.",
    "Sonara uses Kokoro and Claude, not Codex.",
    "Hello, world!",
];

fn render(p: &Phonemizer, input: &str) -> Value {
    let ps = p.phonemize(input);
    json!({
        "text": input,
        "normalized": text::normalize(input),
        "phonemes": ps,
        "batches": phonemes::batches(&ps).len(),
    })
}

#[test]
fn g2p_golden_cases() {
    let p = Phonemizer::new();
    let got: Vec<Value> = CASES.iter().map(|c| render(&p, c)).collect();
    if std::env::var_os("SONARA_BLESS").is_some() {
        let text = serde_json::to_string_pretty(&got).unwrap() + "\n";
        std::fs::write(FIXTURE, text).unwrap();
        return;
    }
    let want: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(FIXTURE).expect("run with SONARA_BLESS=1 to create the fixture"),
    )
    .unwrap();
    assert_eq!(
        got.len(),
        want.len(),
        "case count; bless after adding cases"
    );
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
}

#[test]
fn every_phoneme_is_a_kokoro_symbol() {
    let p = Phonemizer::new();
    for c in CASES {
        let ps = p.phonemize(c);
        let unknown: String = ps
            .chars()
            .filter(|ch| sonara_engine::kokoro::vocab::id(*ch).is_none())
            .collect();
        assert!(unknown.is_empty(), "{c:?} -> {ps:?} has {unknown:?}");
        assert!(!ps.contains('\u{2753}'), "{c:?} -> {ps:?}");
    }
}

#[test]
fn custom_words_are_read_as_sonara_says_them() {
    let p = Phonemizer::new();
    assert_eq!(p.phonemize("Sonara"), "sənˈɑɹə");
    assert_eq!(p.phonemize("Kokoro"), "kˈOkəɹˌO");
    assert!(p.phonemize("onnx").contains("ˈɑnɪks"));
}

#[test]
fn unicode_and_long_text_never_panic() {
    let p = Phonemizer::new();
    for t in [
        "Caf\u{e9} \u{2192} na\u{ef}ve \u{1f600} \u{4e2d}\u{6587} \u{201c}quoted\u{201d}",
        "\u{200d}\u{200d}",
        "...",
        "",
    ] {
        let _ = p.phonemize(t);
    }
    let long = "Sonara reads the latest message aloud. ".repeat(200);
    let ps = p.phonemize(&long);
    let batches = phonemes::batches(&ps);
    assert!(batches.len() >= 200, "{}", batches.len());
    assert!(batches
        .iter()
        .all(|b| phonemes::tokens(b).len() <= phonemes::MAX_TOKENS));
}

#[test]
fn vocab_symbols_are_the_kokoro_set() {
    let symbols: String = sonara_engine::kokoro::vocab::VOCAB
        .iter()
        .map(|(c, _)| *c)
        .collect();
    assert_eq!(
        symbols,
        ";:,.!?\u{2014}\u{2026}\"()\u{201c}\u{201d} \u{303}ʣʥʦʨᵝꭧAIOQSTWYᵊabcdefhijklmnopqrstuvwxyzɑɐɒæβɔɕçɖðʤəɚɛɜɟɡɥɨɪʝɯɰŋɳɲɴøɸθœɹɾɻʁɽʂʃʈʧʊʋʌɣɤχʎʒʔˈˌːʰʲ↓→↗↘ᵻ"
    );
    assert_eq!(sonara_engine::kokoro::vocab::VOCAB.len(), 114);
}
