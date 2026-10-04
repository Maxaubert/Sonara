//! A provider's response body to PCM (spec 13.1 "Audio parsing"): WAV goes
//! through `wav::decode` (float, LIST chunks, placeholder sizes), anything
//! else is raw 16-bit little-endian mono PCM, unless it is plainly another
//! format (MP3, Ogg, FLAC, JSON, HTML), which is a `format` error: Sonara
//! has no decoder for compressed audio and never plays an error page. When
//! the adapter asked for raw PCM, only the reply's `Content-Type` can say
//! it is compressed: headerless samples often start with bytes that look
//! like a magic number (sample -1 is an MP3 frame sync).
use super::error::ExtError;
use crate::{PcmChunk, Reason};

/// Used when neither the reply nor the request names a rate.
pub const DEFAULT_RATE: u32 = 24_000;

/// The `rate=` parameter of a `Content-Type` such as
/// `audio/pcm;rate=24000` or `audio/L16; rate=16000; channels=1`.
pub fn rate_from_content_type(ct: &str) -> Option<u32> {
    ct.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        let k = k.trim().to_ascii_lowercase();
        (k == "rate" || k == "samplerate" || k == "sample_rate")
            .then(|| v.trim().trim_matches('"').parse().ok())
            .flatten()
    })
}

/// A compressed format the reply's `Content-Type` names.
fn named_compressed(ct: &str) -> Option<&'static str> {
    let mime = ct.split(';').next().unwrap_or_default().trim();
    let mime = mime.to_ascii_lowercase();
    if mime.contains("mpeg") || mime.contains("mp3") {
        Some("MP3")
    } else if mime.contains("ogg") || mime.contains("opus") {
        Some("Ogg")
    } else if mime.contains("flac") {
        Some("FLAC")
    } else {
        None
    }
}

/// A full MPEG audio frame header: sync, a version and layer that are not
/// reserved, a bitrate index that is not 15, a sample-rate index not 3.
fn mp3_frame(body: &[u8]) -> bool {
    let [a, b, c, ..] = body else { return false };
    *a == 0xFF
        && b & 0xE0 == 0xE0
        && (b >> 3) & 3 != 1
        && (b >> 1) & 3 != 0
        && c >> 4 != 0xF
        && (c >> 2) & 3 != 3
}

fn sniff_compressed(body: &[u8]) -> Option<&'static str> {
    if body.starts_with(b"ID3") || mp3_frame(body) {
        Some("MP3")
    } else if body.starts_with(b"OggS") {
        Some("Ogg")
    } else if body.starts_with(b"fLaC") {
        Some("FLAC")
    } else {
        None
    }
}

fn sniff_text(body: &[u8]) -> Option<&'static str> {
    let first = body.iter().copied().find(|b| !b.is_ascii_whitespace());
    // Text, not samples: `{"` / `{}` (JSON) or `<` and a tag start (HTML,
    // XML). Raw PCM may start with these bytes too, so the next byte must
    // also fit.
    let text = String::from_utf8_lossy(&body[..body.len().min(16)]);
    let mut chars = text.trim_start().chars();
    match (first, chars.next(), chars.find(|c| !c.is_whitespace())) {
        (Some(b'{'), _, Some('"' | '}')) => Some("JSON"),
        (Some(b'<'), _, Some(c)) if c.is_ascii_alphabetic() || c == '!' || c == '?' => {
            Some("HTML or XML")
        }
        _ => None,
    }
}

/// Streamed WAVs carry 0 or 0xFFFFFFFF as the data size: point it at the
/// bytes that are there, so `wav::decode` reads them all.
fn fix_streamed_sizes(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut at = 12;
    while at + 8 <= out.len() {
        let size =
            u32::from_le_bytes([out[at + 4], out[at + 5], out[at + 6], out[at + 7]]) as usize;
        if &out[at..at + 4] == b"data" {
            let present = out.len() - (at + 8);
            if size == 0 || size > present {
                out[at + 4..at + 8].copy_from_slice(&(present as u32).to_le_bytes());
            }
            break;
        }
        at = at + 8 + size + (size & 1);
    }
    out
}

/// Decode a 2xx body. `content_type` is the reply's header, `requested`
/// the rate Sonara asked for (raw PCM only), `raw_pcm` whether the request
/// asked for headerless PCM (then the body's first bytes are not sniffed
/// for compressed formats).
pub fn decode_body(
    body: &[u8],
    content_type: Option<&str>,
    requested: Option<u32>,
    raw_pcm: bool,
    label: &str,
) -> Result<PcmChunk, ExtError> {
    if body.is_empty() {
        return Err(ExtError::new(
            Reason::Format,
            format!("{label} sent no audio"),
        ));
    }
    if body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WAVE") {
        let pcm = crate::wav::decode(&fix_streamed_sizes(body)).map_err(|e| {
            ExtError::new(
                Reason::Format,
                format!("{label} sent a WAV Sonara cannot read: {e}"),
            )
        })?;
        return Ok(to_mono(pcm));
    }
    let compressed = match content_type.and_then(named_compressed) {
        Some(kind) => Some(kind),
        None if raw_pcm => None,
        None => sniff_compressed(body),
    };
    if let Some(kind) = compressed.or_else(|| sniff_text(body)) {
        return Err(ExtError::new(
            Reason::Format,
            format!("{label} sent {kind}, not WAV/PCM; set response_format to wav"),
        ));
    }
    let rate = content_type
        .and_then(rate_from_content_type)
        .or(requested)
        .unwrap_or(DEFAULT_RATE);
    let (pairs, _) = body.as_chunks::<2>();
    Ok(PcmChunk {
        samples: pairs.iter().map(|p| i16::from_le_bytes(*p)).collect(),
        sample_rate: rate,
        channels: 1,
    })
}

/// Mix to mono (providers send mono; a stereo WAV from a local server is
/// averaged).
fn to_mono(pcm: PcmChunk) -> PcmChunk {
    if pcm.channels <= 1 {
        return pcm;
    }
    let ch = pcm.channels as usize;
    let samples = pcm
        .samples
        .chunks(ch)
        .filter(|f| f.len() == ch)
        .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / ch as i32) as i16)
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
    use crate::wav;

    fn pcm(samples: Vec<i16>, rate: u32) -> PcmChunk {
        PcmChunk {
            samples,
            sample_rate: rate,
            channels: 1,
        }
    }

    fn decode(body: &[u8], ct: Option<&str>) -> Result<PcmChunk, ExtError> {
        decode_body(body, ct, Some(24_000), false, "Test")
    }

    fn decode_raw(body: &[u8], ct: Option<&str>) -> Result<PcmChunk, ExtError> {
        decode_body(body, ct, Some(24_000), true, "Test")
    }

    #[test]
    fn requested_raw_pcm_starting_with_minus_one_is_pcm() {
        // -1 (FF FF) looks like an MP3 frame sync; 0x0090 then makes the
        // next bytes a valid frame header.
        for samples in [vec![-1i16, 0, -2], vec![-1, 0x0090, 5], vec![-5, 1]] {
            let body: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            for ct in [None, Some("audio/pcm"), Some("application/octet-stream")] {
                assert_eq!(decode_raw(&body, ct).unwrap().samples, samples, "{ct:?}");
            }
        }
        // Magic numbers of other formats are samples too.
        assert!(decode_raw(b"ID3\x04", None).is_ok());
        assert!(decode_raw(b"OggS", None).is_ok());
    }

    #[test]
    fn requested_raw_pcm_still_refuses_named_compressed_audio_and_text() {
        for ct in ["audio/mpeg", "audio/ogg; codecs=opus", "audio/flac"] {
            let e = decode_raw(&[0xFF, 0xFF, 0, 0], Some(ct)).unwrap_err();
            assert_eq!(e.reason, Reason::Format, "{ct}");
        }
        assert_eq!(
            decode_raw(br#"{"error": "x"}"#, None).unwrap_err().reason,
            Reason::Format
        );
    }

    #[test]
    fn mp3_sniff_needs_a_valid_frame_header() {
        // Bitrate index 15 and sample-rate index 3 are not MP3.
        assert!(decode(&[0xFF, 0xFB, 0xF0, 0x00], None).is_ok());
        assert!(decode(&[0xFF, 0xFB, 0x9C, 0x00], None).is_ok());
        // Layer 0 and version 1 are reserved.
        assert!(decode(&[0xFF, 0xF9, 0x90, 0x00], None).is_ok());
        assert!(decode(&[0xFF, 0xEB, 0x90, 0x00], None).is_ok());
    }

    #[test]
    fn riff_with_list_chunk() {
        let plain = wav::encode(&pcm(vec![1, 2, 3], 22_050));
        // Insert a LIST chunk between fmt and data.
        let mut with_list = plain[..36].to_vec();
        with_list.extend(b"LIST");
        with_list.extend(5u32.to_le_bytes());
        with_list.extend(b"INFOx\0");
        with_list.extend(&plain[36..]);
        let out = decode(&with_list, Some("audio/wav")).unwrap();
        assert_eq!(out, pcm(vec![1, 2, 3], 22_050));
    }

    #[test]
    fn streamed_wav_placeholder_sizes() {
        for placeholder in [0u32, u32::MAX] {
            let mut b = wav::encode(&pcm(vec![5, -5, 7, -7], 24_000));
            b[4..8].copy_from_slice(&placeholder.to_le_bytes());
            b[40..44].copy_from_slice(&placeholder.to_le_bytes());
            let out = decode(&b, None).unwrap();
            assert_eq!(out.samples, vec![5, -5, 7, -7], "{placeholder:#x}");
        }
    }

    #[test]
    fn float32_wav() {
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend(36u32.to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(3u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(24_000u32.to_le_bytes());
        b.extend((24_000u32 * 4).to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(32u16.to_le_bytes());
        b.extend(b"data");
        b.extend(8u32.to_le_bytes());
        b.extend(0.5f32.to_le_bytes());
        b.extend((-1.0f32).to_le_bytes());
        let out = decode(&b, Some("audio/wav")).unwrap();
        assert_eq!(out.samples, vec![16_384, -32_768]);
        assert_eq!(out.sample_rate, 24_000);
    }

    #[test]
    fn raw_pcm_rate_from_content_type() {
        let body: Vec<u8> = [1i16, -2, 3].iter().flat_map(|s| s.to_le_bytes()).collect();
        let out = decode(&body, Some("audio/pcm; rate=16000; channels=1")).unwrap();
        assert_eq!(out, pcm(vec![1, -2, 3], 16_000));
        let out = decode(&body, Some("application/octet-stream")).unwrap();
        assert_eq!(out.sample_rate, 24_000, "the requested rate");
        let out = decode_body(&body, None, None, false, "T").unwrap();
        assert_eq!(out.sample_rate, DEFAULT_RATE);
    }

    #[test]
    fn odd_trailing_byte_dropped() {
        let out = decode(&[1, 0, 2, 0, 9], None).unwrap();
        assert_eq!(out.samples, vec![1, 2]);
    }

    #[test]
    fn mp3_body_is_format_error() {
        for body in [
            &b"ID3\x04\0\0\0\0"[..],
            &[0xFF, 0xFB, 0x90, 0x00],
            b"OggS\0\0",
            b"fLaC\0",
        ] {
            let e = decode(body, Some("audio/mpeg")).unwrap_err();
            assert_eq!(e.reason, Reason::Format);
        }
        assert!(decode(b"ID3\x04", None)
            .unwrap_err()
            .message
            .contains("set response_format"));
    }

    #[test]
    fn json_body_is_format_error() {
        for body in [
            &br#"{"error": "nope"}"#[..],
            b"  {}",
            b"<html>oops</html>",
            b"<?xml",
            b"",
        ] {
            assert_eq!(decode(body, None).unwrap_err().reason, Reason::Format);
        }
    }

    #[test]
    fn pcm_that_starts_like_text_is_still_pcm() {
        // Samples 0x7B (`{`) and 0x3C (`<`) followed by binary bytes.
        let out = decode(&[b'{', 0, 1, 0], None).unwrap();
        assert_eq!(out.samples, vec![0x7B, 1]);
        assert!(decode(&[b'<', 0x80, 1, 0], None).is_ok());
    }

    #[test]
    fn stereo_wav_is_mixed_to_mono() {
        let b = wav::encode(&PcmChunk {
            samples: vec![100, 300, -10, -30],
            sample_rate: 24_000,
            channels: 2,
        });
        assert_eq!(decode(&b, None).unwrap(), pcm(vec![200, -20], 24_000));
    }
}
