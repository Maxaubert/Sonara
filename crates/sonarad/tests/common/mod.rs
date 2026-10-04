//! Shared by the integration tests: a small local HTTP server standing in
//! for an OpenAI-compatible speech provider, which keeps every request.
#![allow(dead_code)]
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One request the provider saw: path, headers (lower-case names), body.
pub type Seen = (String, Vec<(String, String)>, Vec<u8>);
/// Headers and body of a request.
pub type Request = (Vec<(String, String)>, Vec<u8>);

/// A provider: every POST answers `answer`; requests are kept.
#[derive(Clone)]
pub struct Provider {
    pub url: String,
    pub answer: Arc<Mutex<(u16, &'static str, Vec<u8>)>>,
    /// How long a speech reply waits before it is sent.
    pub delay: Arc<Mutex<Duration>>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

pub fn wav(samples: usize) -> Vec<u8> {
    sonara_engine::wav::encode(&sonara_engine::PcmChunk {
        samples: vec![1000; samples],
        sample_rate: 24_000,
        channels: 1,
    })
}

impl Provider {
    pub fn start() -> Provider {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = Provider {
            url: format!("http://{}/v1", listener.local_addr().unwrap()),
            answer: Arc::new(Mutex::new((200, "audio/wav", wav(2400)))),
            delay: Arc::new(Mutex::new(Duration::ZERO)),
            seen: Arc::new(Mutex::new(Vec::new())),
        };
        let q = p.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let q = q.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(conn.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap_or(0);
                    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut headers = Vec::new();
                    let mut len = 0;
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        if let Some((k, v)) = l.split_once(':') {
                            let k = k.trim().to_ascii_lowercase();
                            if k == "content-length" {
                                len = v.trim().parse().unwrap_or(0);
                            }
                            headers.push((k, v.trim().to_string()));
                        }
                    }
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    q.seen.lock().unwrap().push((path.clone(), headers, body));
                    let (status, ct, b) = if path.ends_with("/audio/voices") {
                        (
                            200,
                            "application/json",
                            br#"{"voices": ["af_heart", "am_echo"]}"#.to_vec(),
                        )
                    } else {
                        let delay = *q.delay.lock().unwrap();
                        std::thread::sleep(delay);
                        q.answer.lock().unwrap().clone()
                    };
                    let mut out = conn;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = out.write_all(&b);
                });
            }
        });
        p
    }

    pub fn fail(&self, status: u16, body: &str) {
        *self.answer.lock().unwrap() = (status, "application/json", body.as_bytes().to_vec());
    }

    pub fn speech_requests(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _, _)| p.ends_with("/audio/speech"))
            .map(|(_, h, b)| (h.clone(), b.clone()))
            .collect()
    }
}
