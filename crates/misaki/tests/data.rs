//! The compressed data decodes to exactly the upstream misaki-rs 0.6.0
//! files (the SHA-256 values pinned in `tools/pack_data.py`).
use misaki::{G2P, Language, Lexicon};
use std::sync::Arc;

fn sha256(text: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

#[test]
fn lexicon_compression_round_trips_to_the_upstream_bytes() {
    let us_gold = include_bytes!("../data/us_gold.json.xz");
    let us_silver = include_bytes!("../data/us_silver.json.xz");
    let weights = include_bytes!("../data/tagger-weights.json.xz");
    let tags = include_bytes!("../data/tagger-tags.json.xz");
    for (packed, sha) in [
        (
            &us_gold[..],
            "bb83c899d8dbfa160fa05661bea052bacfeece9b639851662334e85002ee8ad9",
        ),
        (
            &us_silver[..],
            "57cae2a1a9d73ce219ad9142b0d904914a0228cb1babce20e5bfd4e1b1307ee4",
        ),
        (
            &weights[..],
            "789e8e35f6fac656d9b7eccea29ba4e8a137cbb5b525dd31f39e4291a9e80832",
        ),
        (
            &tags[..],
            "4c713731cb06727736962bc8cd94ad919217a040e76691059af99bc0c2c9c246",
        ),
    ] {
        assert_eq!(sha256(&misaki::data::unxz(packed)), sha);
    }
    assert_eq!(
        sha256(misaki::data::TAGGER_CLASSES),
        "7812cb7945022cb348bc6670b3eb7c76cdd50920715e86681337adfbf9c85253"
    );
}

#[test]
fn the_compressed_data_is_much_smaller_than_the_json() {
    let packed = misaki::data::US_GOLD_XZ.len() + misaki::data::US_SILVER_XZ.len();
    // 15.1 MB of US lexicon JSON upstream.
    assert!(packed < 3_000_000, "{packed} bytes");
}

#[test]
fn a_host_word_wins_over_the_lexicon() {
    let mut lexicon = Lexicon::new(Language::EnglishUS);
    lexicon.insert_gold("Sonara", "sənˈɑɹə");
    let g2p = G2P::with_lexicon(Arc::new(lexicon), None);
    let (ps, _) = g2p.g2p("Sonara").unwrap();
    assert_eq!(ps.trim(), "sənˈɑɹə");
}
