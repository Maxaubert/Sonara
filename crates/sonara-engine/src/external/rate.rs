//! Sonara's rate (words per minute) as each provider's speed (spec 9):
//! `s = wpm / 200` (Kokoro's baseline), clamped to what the provider takes.
use super::profile::Kind;

/// The plain factor: 100..=400 wpm is 0.5..=2.0.
pub fn factor(wpm: u32) -> f64 {
    wpm as f64 / 200.0
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The speed value a kind sends for `wpm` (`None`: the parameter is not
/// sent). Two decimals.
pub fn speed(kind: Kind, wpm: u32) -> Option<f64> {
    let s = factor(wpm);
    let (lo, hi) = match kind {
        Kind::OpenAiCompatible => (0.25, 4.0),
        Kind::ElevenLabs => (0.7, 1.2),
        Kind::Azure => (0.5, 2.0),
        Kind::Google => (0.25, 2.0),
        Kind::Cartesia => (0.6, 1.5),
        // Uncertain range (spec 13.3): omitted at 1.0.
        Kind::Deepgram => {
            if (s - 1.0).abs() < f64::EPSILON {
                return None;
            }
            (0.7, 1.5)
        }
        Kind::Command => return Some(round2(s)),
    };
    Some(round2(s.clamp(lo, hi)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_at_100_200_250_400_wpm() {
        let table: [(Kind, [Option<f64>; 4]); 7] = [
            (
                Kind::OpenAiCompatible,
                [Some(0.5), Some(1.0), Some(1.25), Some(2.0)],
            ),
            (
                Kind::ElevenLabs,
                [Some(0.7), Some(1.0), Some(1.2), Some(1.2)],
            ),
            (Kind::Azure, [Some(0.5), Some(1.0), Some(1.25), Some(2.0)]),
            (Kind::Google, [Some(0.5), Some(1.0), Some(1.25), Some(2.0)]),
            (
                Kind::Cartesia,
                [Some(0.6), Some(1.0), Some(1.25), Some(1.5)],
            ),
            (Kind::Deepgram, [Some(0.7), None, Some(1.25), Some(1.5)]),
            (Kind::Command, [Some(0.5), Some(1.0), Some(1.25), Some(2.0)]),
        ];
        for (kind, want) in table {
            let got: Vec<Option<f64>> = [100, 200, 250, 400].map(|w| speed(kind, w)).to_vec();
            assert_eq!(got, want.to_vec(), "{kind:?}");
        }
        assert_eq!(speed(Kind::ElevenLabs, 140), Some(0.7));
        assert_eq!(speed(Kind::ElevenLabs, 240), Some(1.2));
        assert_eq!(speed(Kind::OpenAiCompatible, 333), Some(1.67));
    }
}
