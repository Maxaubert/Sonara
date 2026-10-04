//! The cue cache (spec 7.4): provider audio of short texts ("Paused.",
//! "Rate 250.") kept in memory, so a repeated cue costs no round trip. Only
//! provider audio goes in; fallback audio never does.
use crate::PcmChunk;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Texts up to this many characters are cached.
pub const MAX_TEXT_CHARS: usize = 64;
/// Entries kept (least recently used out first).
pub const CAPACITY: usize = 128;

type Key = (String, u32, String);

#[derive(Default)]
pub struct CueCache {
    entries: Mutex<VecDeque<(Key, Vec<PcmChunk>)>>,
}

impl CueCache {
    pub fn new() -> CueCache {
        CueCache::default()
    }

    pub fn cacheable(text: &str) -> bool {
        text.chars().count() <= MAX_TEXT_CHARS
    }

    pub fn get(&self, voice: &str, rate: u32, text: &str) -> Option<Vec<PcmChunk>> {
        if !Self::cacheable(text) {
            return None;
        }
        let mut e = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let at = e
            .iter()
            .position(|(k, _)| k.0 == voice && k.1 == rate && k.2 == text)?;
        let hit = e.remove(at)?;
        let pcm = hit.1.clone();
        e.push_back(hit);
        Some(pcm)
    }

    pub fn put(&self, voice: &str, rate: u32, text: &str, pcm: Vec<PcmChunk>) {
        if !Self::cacheable(text) {
            return;
        }
        let mut e = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        e.retain(|(k, _)| !(k.0 == voice && k.1 == rate && k.2 == text));
        e.push_back(((voice.to_string(), rate, text.to_string()), pcm));
        while e.len() > CAPACITY {
            e.pop_front();
        }
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(n: i16) -> Vec<PcmChunk> {
        vec![PcmChunk {
            samples: vec![n],
            sample_rate: 24_000,
            channels: 1,
        }]
    }

    #[test]
    fn keyed_by_voice_rate_and_text() {
        let c = CueCache::new();
        c.put("v1", 250, "Paused.", pcm(1));
        assert_eq!(c.get("v1", 250, "Paused."), Some(pcm(1)));
        assert_eq!(c.get("v1", 200, "Paused."), None);
        assert_eq!(c.get("v2", 250, "Paused."), None);
        assert_eq!(c.get("v1", 250, "Paused"), None);
        c.put("v1", 250, "Paused.", pcm(2));
        assert_eq!(c.get("v1", 250, "Paused."), Some(pcm(2)));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn long_texts_are_not_cached() {
        let c = CueCache::new();
        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        c.put("v", 200, &long, pcm(1));
        assert!(c.is_empty());
        let edge = "x".repeat(MAX_TEXT_CHARS);
        c.put("v", 200, &edge, pcm(1));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn least_recently_used_goes_first() {
        let c = CueCache::new();
        for i in 0..CAPACITY {
            c.put("v", 200, &format!("t{i}"), pcm(i as i16));
        }
        assert!(c.get("v", 200, "t0").is_some(), "t0 is now the newest");
        c.put("v", 200, "new", pcm(0));
        assert_eq!(c.len(), CAPACITY);
        assert!(c.get("v", 200, "t1").is_none(), "t1 was the oldest");
        assert!(c.get("v", 200, "t0").is_some());
        c.clear();
        assert!(c.is_empty());
    }
}
