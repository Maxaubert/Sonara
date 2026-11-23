//! "the" before a word follows that word's first sound, not its first
//! letter, as in Python misaki (audit 2026-11-23): "the hour" uses the
//! pre-vowel form, "the user" the pre-consonant form.
use misaki::{G2P, Language};

fn phonemes(g2p: &G2P, text: &str) -> String {
    g2p.g2p(text).unwrap().0
}

/// The phonemes misaki gives "the" in `text` (its first word).
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
