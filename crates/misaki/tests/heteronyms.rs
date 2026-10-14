//! Words said two ways (heteronyms): the part-of-speech tag picks the
//! pronunciation, as in misaki's own lexicon (owner's report 2026-10-14:
//! "it's being read" was read "reed", not "red").
use misaki::{G2P, Language};

fn phonemes(g2p: &G2P, text: &str) -> String {
    g2p.g2p(text).unwrap().0
}

#[test]
fn read_is_said_red_in_the_past_and_reed_otherwise() {
    // "Please" first: the tagger reads its context in lower case, as NLTK
    // trained it (it tagged "read" VBD after a capitalized "Please").
    let g2p = G2P::new(Language::EnglishUS);
    for past in [
        "It's being read.",
        "I have read the book.",
        "The file was read twice.",
        "Each sentence is read separately.",
    ] {
        assert!(
            phonemes(&g2p, past).contains("ɹˈɛd"),
            "{past}: {}",
            phonemes(&g2p, past)
        );
    }
    for present in ["I will read it.", "Read the file.", "Please read this."] {
        assert!(
            phonemes(&g2p, present).contains("ɹˈid"),
            "{present}: {}",
            phonemes(&g2p, present)
        );
    }
}

#[test]
fn other_heteronyms_follow_their_part_of_speech() {
    let g2p = G2P::new(Language::EnglishUS);
    // (sentence, a pronunciation it must contain)
    for (text, want) in [
        ("We live here.", "lˈɪv"),
        ("The show is live.", "lˈIv"),
        ("Please record the call.", "ɹəkˈɔɹd"),
        ("The record is new.", "ɹˈɛkəɹd"),
        ("Please close the door.", "klˈOz"),
        ("They live close to us.", "klˈOs"),
        ("Do not tear it.", "tˈɛɹ"),
        ("The tears are real.", "tˈɪɹz"),
    ] {
        let ps = phonemes(&g2p, text);
        assert!(ps.contains(want), "{text}: {ps}, want {want}");
    }
}
