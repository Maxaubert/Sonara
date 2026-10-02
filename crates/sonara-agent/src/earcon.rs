//! Earcons: short tones that mark agent events (a question, a permission
//! prompt, the end of a turn...). The WAVs are the Python plugin's bundled
//! ones (`src/sonara/platform/windows/earcons`, made by its `generate.py`),
//! compiled in, and played through the L1 output's clip mixer so they never
//! pause or cut speech.
use sonara_engine::{wav, PcmChunk};
use std::sync::OnceLock;

macro_rules! earcons {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// The earcon kinds (spec section 8).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Earcon {
            $($variant),*
        }

        impl Earcon {
            pub const ALL: &'static [Earcon] = &[$(Earcon::$variant),*];

            /// The protocol name (`earcon {kind}`).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Earcon::$variant => $name),*
                }
            }

            fn wav(&self) -> &'static [u8] {
                match self {
                    $(Earcon::$variant => include_bytes!(concat!(
                        "../../../src/sonara/platform/windows/earcons/",
                        $name,
                        ".wav"
                    ))),*
                }
            }
        }
    };
}

earcons! {
    Choice => "choice",
    Permission => "permission",
    Error => "error",
    TurnDone => "turn_done",
    Nav => "nav",
    NavEdge => "nav_edge",
    SessionChange => "session_change",
    SummaryFailed => "summary_failed",
}

impl Earcon {
    pub fn parse(name: &str) -> Option<Earcon> {
        Earcon::ALL.iter().copied().find(|e| e.as_str() == name)
    }

    /// The decoded clip, mono 16-bit (decoded once).
    pub fn clip(&self) -> &'static PcmChunk {
        static CLIPS: OnceLock<Vec<PcmChunk>> = OnceLock::new();
        let clips = CLIPS.get_or_init(|| {
            Earcon::ALL
                .iter()
                .map(|e| {
                    let pcm = wav::decode(e.wav()).expect("a bundled earcon is a valid WAV");
                    mono(pcm)
                })
                .collect()
        });
        let i = Earcon::ALL
            .iter()
            .position(|e| e == self)
            .expect("every earcon is listed");
        &clips[i]
    }
}

/// Mix down to one channel (the bundled WAVs are mono already).
fn mono(pcm: PcmChunk) -> PcmChunk {
    let n = pcm.channels.max(1) as usize;
    if n == 1 {
        return pcm;
    }
    let samples = pcm
        .samples
        .chunks(n)
        .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / f.len() as i32) as i16)
        .collect();
    PcmChunk {
        samples,
        sample_rate: pcm.sample_rate,
        channels: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_earcon_decodes_to_a_short_mono_clip() {
        for e in Earcon::ALL {
            let c = e.clip();
            assert_eq!(c.channels, 1, "{e:?}");
            assert!(c.sample_rate >= 8_000, "{e:?}");
            let secs = c.samples.len() as f32 / c.sample_rate as f32;
            assert!(secs > 0.01 && secs < 2.0, "{e:?} lasts {secs} s");
            assert!(c.samples.iter().any(|&s| s != 0), "{e:?} is silent");
        }
    }

    #[test]
    fn names_round_trip() {
        for e in Earcon::ALL {
            assert_eq!(Earcon::parse(e.as_str()), Some(*e));
        }
        assert_eq!(Earcon::parse("ready"), None);
        assert_eq!(Earcon::TurnDone.as_str(), "turn_done");
    }

    #[test]
    fn stereo_is_mixed_down() {
        let pcm = PcmChunk {
            samples: vec![10, 20, -4, 4],
            sample_rate: 8_000,
            channels: 2,
        };
        assert_eq!(mono(pcm).samples, [15, 0]);
    }
}
