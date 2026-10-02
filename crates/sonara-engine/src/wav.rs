//! Decode the WAV container WinRT synthesis returns into PCM.
//!
//! Accepted: RIFF/WAVE with 8-, 16-, 24- or 32-bit integer PCM or 32- or
//! 64-bit IEEE float (plain or WAVE_FORMAT_EXTENSIBLE with the PCM or float
//! subformat), any channel count and sample rate. Every format is converted
//! to 16-bit samples (engines write 16-bit; user earcons may be anything a
//! sound editor saves). Unknown
//! chunks (LIST, fact...) are skipped. A `data` size larger than the bytes
//! present (streamed WAVs write 0 or 0xFFFFFFFF there) is clamped to what is
//! there, and a trailing partial sample is dropped.
use crate::{Error, PcmChunk, Result};

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn bad(msg: &str) -> Error {
    Error::Wav(msg.to_string())
}

struct Format {
    channels: u16,
    sample_rate: u32,
    bits: u16,
    float: bool,
}

fn parse_fmt(body: &[u8]) -> Result<Format> {
    let tag = u16_at(body, 0).ok_or_else(|| bad("fmt chunk too short"))?;
    let channels = u16_at(body, 2).ok_or_else(|| bad("fmt chunk too short"))?;
    let sample_rate = u32_at(body, 4).ok_or_else(|| bad("fmt chunk too short"))?;
    let bits = u16_at(body, 14).ok_or_else(|| bad("fmt chunk too short"))?;
    let kind = match tag {
        // The subformat GUID starts with the format tag (offset 24).
        FORMAT_EXTENSIBLE => u16_at(body, 24).unwrap_or(0),
        other => other,
    };
    let float = match kind {
        FORMAT_PCM => false,
        FORMAT_FLOAT => true,
        _ => return Err(Error::Wav(format!("unsupported format tag {tag:#06x}"))),
    };
    if channels == 0 || sample_rate == 0 {
        return Err(bad("zero channels or sample rate"));
    }
    let supported = if float {
        bits == 32 || bits == 64
    } else {
        matches!(bits, 8 | 16 | 24 | 32)
    };
    if !supported {
        return Err(Error::Wav(format!("unsupported sample size {bits} bits")));
    }
    Ok(Format {
        channels,
        sample_rate,
        bits,
        float,
    })
}

/// Decode a whole WAV file held in memory.
pub fn decode(bytes: &[u8]) -> Result<PcmChunk> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(bad("not a RIFF/WAVE file"));
    }
    let mut at = 12;
    let mut format: Option<Format> = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4).unwrap_or(0) as usize;
        let start = at + 8;
        let end = start.saturating_add(size).min(bytes.len());
        let body = &bytes[start..end];
        if id == b"fmt " {
            format = Some(parse_fmt(body)?);
        } else if id == b"data" {
            let f = format.ok_or_else(|| bad("data chunk before fmt chunk"))?;
            return Ok(to_pcm(&f, body));
        }
        // Chunks are word aligned: an odd size is followed by a pad byte.
        at = start.saturating_add(size).saturating_add(size & 1);
    }
    Err(bad("no data chunk"))
}

/// A float sample (full scale -1.0 to 1.0) as 16-bit, clamped.
fn from_float(x: f64) -> i16 {
    if x.is_nan() {
        return 0;
    }
    (x * 32768.0)
        .round()
        .clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

fn to_pcm(f: &Format, body: &[u8]) -> PcmChunk {
    // A trailing partial sample is dropped (`as_chunks` leaves it out).
    let samples = match (f.float, f.bits) {
        (false, 8) => {
            // 8-bit WAV is unsigned with 128 as silence.
            body.iter().map(|&s| (s as i16 - 128) << 8).collect()
        }
        (false, 16) => {
            let (pairs, _) = body.as_chunks::<2>();
            pairs.iter().map(|p| i16::from_le_bytes(*p)).collect()
        }
        (false, 24) => {
            // The top two bytes of a little-endian 24-bit sample.
            let (triples, _) = body.as_chunks::<3>();
            triples
                .iter()
                .map(|t| i16::from_le_bytes([t[1], t[2]]))
                .collect()
        }
        (false, _) => {
            let (quads, _) = body.as_chunks::<4>();
            quads
                .iter()
                .map(|q| i16::from_le_bytes([q[2], q[3]]))
                .collect()
        }
        (true, 32) => {
            let (quads, _) = body.as_chunks::<4>();
            quads
                .iter()
                .map(|q| from_float(f64::from(f32::from_le_bytes(*q))))
                .collect()
        }
        (true, _) => {
            let (eights, _) = body.as_chunks::<8>();
            eights
                .iter()
                .map(|e| from_float(f64::from_le_bytes(*e)))
                .collect()
        }
    };
    PcmChunk {
        samples,
        sample_rate: f.sample_rate,
        channels: f.channels,
    }
}

/// Encode 16-bit PCM as a canonical WAV file (for tests and fixtures).
pub fn encode(pcm: &PcmChunk) -> Vec<u8> {
    let data_len = (pcm.samples.len() * 2) as u32;
    let block_align = pcm.channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&pcm.channels.to_le_bytes());
    out.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    out.extend_from_slice(&(pcm.sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &pcm.samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header of a 22050 Hz mono 16-bit WAV as OneCore writes it,
    /// followed by four samples: 0, 1, -1, 32767.
    const ONECORE_MONO: [u8; 52] = [
        b'R', b'I', b'F', b'F', 44, 0, 0, 0, b'W', b'A', b'V', b'E', //
        b'f', b'm', b't', b' ', 16, 0, 0, 0, //
        1, 0, 1, 0, 0x22, 0x56, 0, 0, 0x44, 0xAC, 0, 0, 2, 0, 16, 0, //
        b'd', b'a', b't', b'a', 8, 0, 0, 0, //
        0, 0, 1, 0, 0xFF, 0xFF, 0xFF, 0x7F,
    ];

    #[test]
    fn decodes_onecore_mono_16_bit() {
        let pcm = decode(&ONECORE_MONO).unwrap();
        assert_eq!(pcm.sample_rate, 22050);
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.samples, vec![0, 1, -1, 32767]);
    }

    #[test]
    fn decodes_stereo_and_round_trips_encode() {
        let pcm = PcmChunk {
            samples: vec![1, -2, 300, -400, i16::MIN, i16::MAX],
            sample_rate: 48000,
            channels: 2,
        };
        assert_eq!(decode(&encode(&pcm)).unwrap(), pcm);
    }

    #[test]
    fn skips_unknown_chunks_and_odd_padding() {
        let mut b = ONECORE_MONO[..36].to_vec();
        // A LIST chunk of odd size 3, plus its pad byte.
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[9, 9, 9, 0]);
        b.extend_from_slice(&ONECORE_MONO[36..]);
        assert_eq!(decode(&b).unwrap().samples, vec![0, 1, -1, 32767]);
    }

    #[test]
    fn accepts_extensible_pcm() {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&0xFFFEu16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // channels
        b.extend_from_slice(&16000u32.to_le_bytes());
        b.extend_from_slice(&32000u32.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        b.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        b.extend_from_slice(&4u32.to_le_bytes()); // channel mask
        b.extend_from_slice(&1u16.to_le_bytes()); // subformat: PCM
        b.extend_from_slice(&[0; 14]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&7i16.to_le_bytes());
        let pcm = decode(&b).unwrap();
        assert_eq!((pcm.sample_rate, pcm.channels), (16000, 1));
        assert_eq!(pcm.samples, vec![7]);
    }

    /// A mono 8000 Hz WAV of `tag` and `bits` around `data`.
    fn wav_of(tag: u16, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut b = ONECORE_MONO[..44].to_vec();
        b[20..22].copy_from_slice(&tag.to_le_bytes());
        b[34..36].copy_from_slice(&bits.to_le_bytes());
        b[40..44].copy_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn decodes_24_and_32_bit_integer_to_16_bit() {
        // 0x123456 and -1 (24-bit), then 0x7FFFFFFF and i32::MIN (32-bit).
        let pcm = decode(&wav_of(1, 24, &[0x56, 0x34, 0x12, 0xFF, 0xFF, 0xFF])).unwrap();
        assert_eq!(pcm.samples, vec![0x1234, -1]);
        let mut d = 0x7FFF_FFFFi32.to_le_bytes().to_vec();
        d.extend_from_slice(&i32::MIN.to_le_bytes());
        assert_eq!(
            decode(&wav_of(1, 32, &d)).unwrap().samples,
            vec![i16::MAX, i16::MIN]
        );
    }

    #[test]
    fn decodes_float_32_and_64_clamped() {
        let mut d = Vec::new();
        for x in [0.0f32, 0.5, -1.0, 2.0] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        assert_eq!(
            decode(&wav_of(3, 32, &d)).unwrap().samples,
            vec![0, 16384, i16::MIN, i16::MAX]
        );
        let mut d = Vec::new();
        for x in [-0.25f64, f64::NAN] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        assert_eq!(decode(&wav_of(3, 64, &d)).unwrap().samples, vec![-8192, 0]);
    }

    #[test]
    fn decodes_8_bit_unsigned() {
        let mut b = ONECORE_MONO[..44].to_vec();
        b[34] = 8; // bits per sample
        b[40] = 3; // data size
        b.extend_from_slice(&[128, 255, 0]);
        let pcm = decode(&b).unwrap();
        assert_eq!(pcm.samples, vec![0, 127 << 8, -128 << 8]);
    }

    #[test]
    fn clamps_an_oversized_data_chunk_and_drops_a_partial_sample() {
        let mut b = ONECORE_MONO.to_vec();
        b[40..44].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        b.push(0x42); // half a sample
        assert_eq!(decode(&b).unwrap().samples, vec![0, 1, -1, 32767]);
    }

    #[test]
    fn empty_data_is_an_empty_chunk() {
        let mut b = ONECORE_MONO[..44].to_vec();
        b[40] = 0;
        assert!(decode(&b).unwrap().samples.is_empty());
    }

    #[test]
    fn rejects_what_it_cannot_play() {
        let err = |b: &[u8]| match decode(b) {
            Err(Error::Wav(m)) => m,
            other => panic!("expected a WAV error, got {other:?}"),
        };
        assert_eq!(err(b"RIFX\0\0\0\0WAVE"), "not a RIFF/WAVE file");
        assert_eq!(err(&ONECORE_MONO[..10]), "not a RIFF/WAVE file");
        assert_eq!(err(&ONECORE_MONO[..36]), "no data chunk");
        assert_eq!(err(&ONECORE_MONO[..26]), "fmt chunk too short");

        let mut alaw = ONECORE_MONO.to_vec();
        alaw[20] = 6; // A-law
        assert_eq!(err(&alaw), "unsupported format tag 0x0006");

        let mut b12 = ONECORE_MONO.to_vec();
        b12[34] = 12;
        assert_eq!(err(&b12), "unsupported sample size 12 bits");

        let mut f16 = ONECORE_MONO.to_vec();
        f16[20] = 3; // IEEE float, but 16 bits
        assert_eq!(err(&f16), "unsupported sample size 16 bits");

        let mut no_fmt = b"RIFF\0\0\0\0WAVE".to_vec();
        no_fmt.extend_from_slice(&ONECORE_MONO[36..]);
        assert_eq!(err(&no_fmt), "data chunk before fmt chunk");
    }
}
