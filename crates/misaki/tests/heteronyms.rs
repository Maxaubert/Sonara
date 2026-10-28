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

#[test]
fn content_is_the_noun_unless_a_linking_verb_leads_to_it() {
    // Owner's report 2026-10-17: "the content action" was said like
    // "feeling content" (kentent), the adjective.
    let g2p = G2P::new(Language::EnglishUS);
    for noun in [
        "Step 9 is the content action now.",
        "The content action is done.",
        "Check the content of the file.",
        "The content is good.",
        "Content is king.",
        "Markdown content works.",
    ] {
        assert!(
            phonemes(&g2p, noun).contains("kˈɑntɛnt"),
            "{noun}: {}",
            phonemes(&g2p, noun)
        );
    }
    for adjective in [
        "I am content with it.",
        "She seems very content.",
        "I'm content.",
    ] {
        assert!(
            phonemes(&g2p, adjective).contains("kəntˈɛnt"),
            "{adjective}: {}",
            phonemes(&g2p, adjective)
        );
    }
}

#[test]
fn live_after_a_modal_or_subject_is_the_verb() {
    // Owner's report 2026-10-28: "Where should the Bitwarden plugin live?"
    // was said like a live concert (the adjective).
    let g2p = G2P::new(Language::EnglishUS);
    for verb in [
        "Where should the Bitwarden plugin live?",
        "Where should it live?",
        "Where does the config live?",
        "Which folder should the tests live in?",
        "The file will live in docs.",
        "It has to live somewhere.",
        "Let it live there.",
        "Where do they live now?",
        "We live here.",
        "Where the plugin lives is up to you.",
        "The config lives in the home folder.",
    ] {
        let ps = phonemes(&g2p, verb);
        assert!(ps.contains("lˈɪv"), "{verb}: {ps}");
    }
    for adjective in [
        "The show is live.",
        "Should we go live today?",
        "It will be live soon.",
        "Watch the live stream.",
        "A live test runs on this PC.",
        "Run the live tests.",
        "The live check passed.",
        "Is the site live yet?",
        "Seat belts save lives.",
        "Our lives changed.",
    ] {
        let ps = phonemes(&g2p, adjective);
        assert!(ps.contains("lˈIv"), "{adjective}: {ps}");
    }
}
