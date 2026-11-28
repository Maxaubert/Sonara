//! "the" before a word follows that word's first sound, not its first
//! letter, as in Python misaki (audit 2026-11-23): "the hour" uses the
//! pre-vowel form, "the user" the pre-consonant form.
use misaki::{G2P, Language};

fn phonemes(g2p: &G2P, text: &str) -> String {
    g2p.g2p(text).unwrap().0
}

/// The phonemes misaki gives the first word of `text` ("the" or "to").
fn the_form(g2p: &G2P, text: &str) -> String {
    let (_, tokens) = g2p.g2p(text).unwrap();
    tokens[0].phonemes.clone().unwrap()
}

#[test]
fn the_before_a_silent_h_or_a_consonant_sound_follows_the_sound() {
    let g2p = G2P::new(Language::EnglishUS);
    let before_vowel = the_form(&g2p, "the apple");
    let before_consonant = the_form(&g2p, "the dog");
    assert_eq!(before_vowel, "ði");
    assert_eq!(before_consonant, "ðə");

    for text in ["the hour", "the honest man"] {
        assert_eq!(
            the_form(&g2p, text),
            before_vowel,
            "{text}: {}",
            phonemes(&g2p, text)
        );
    }
    for text in ["the user", "the one"] {
        assert_eq!(
            the_form(&g2p, text),
            before_consonant,
            "{text}: {}",
            phonemes(&g2p, text)
        );
    }
}

/// Many lexicon words start with a schwa-like `ɐ` or `ɚ` ("account",
/// "around") or with the bare `e` or `o` of a diphthong ("age", "oak").
#[test]
fn the_and_to_before_every_lexicon_vowel_sound_take_the_pre_vowel_form() {
    let g2p = G2P::new(Language::EnglishUS);
    let mut wrong = Vec::new();
    for (text, expected) in [
        ("the account", "ði"),
        ("the original", "ði"),
        ("the arrival", "ði"),
        ("the oak", "ði"),
        ("the age", "ði"),
        ("to avoid", "tʊ"),
        ("to allow", "tʊ"),
        ("to apply", "tʊ"),
        ("to arrive", "tʊ"),
        ("to an end", "tʊ"),
        ("to open", "tʊ"),
        ("to do", "tə"),
    ] {
        if the_form(&g2p, text) != expected {
            wrong.push(format!("{text}: {}", phonemes(&g2p, text)));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
