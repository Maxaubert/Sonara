//! A number with a possessive 's is one word: "step 8's task" is "step
//! eight's task", not "eight ess" (owner's report 2026-10-17).
use misaki::{G2P, Language};

fn phonemes(text: &str) -> String {
    G2P::new(Language::EnglishUS).g2p(text).unwrap().0
}

#[test]
fn a_possessive_after_a_number_is_said_like_the_spelled_number() {
    for (digits, spelled) in [
        ("Step 8's task is done.", "Step eight's task is done."),
        ("Task 3's output.", "Task three's output."),
        ("Step 12's result.", "Step twelve's result."),
        ("Step 8\u{2019}s task.", "Step eight's task."),
    ] {
        assert_eq!(phonemes(digits), phonemes(spelled), "{digits}");
    }
}

#[test]
fn a_compound_number_ends_in_its_last_word() {
    let ps = phonemes("Item 21's notes.");
    assert!(ps.contains("wˈʌnz") && !ps.contains("ˈɛs"), "{ps}");
}

#[test]
fn the_s_is_not_spelled_as_a_letter() {
    for text in [
        "Step 8's task is done.",
        "Task 3's output.",
        "Step 12's result.",
    ] {
        assert!(
            !phonemes(text).contains("ˈɛs"),
            "{text}: {}",
            phonemes(text)
        );
    }
}

#[test]
fn a_year_with_an_apostrophe_is_left_as_it_was() {
    assert_eq!(
        phonemes("The 1990's were fun."),
        phonemes("The 1990 s were fun.")
    );
}
