//! Test helpers: a temporary folder and a tiny local HTTP file server with
//! scripted faults (no network, no real model).
#![allow(dead_code)]
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// A folder under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sonara-engine-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What the server does with the next request for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Send the headers for the whole answer, then only this many body
    /// bytes, and hang up (a broken connection).
    CutAfter(usize),
    /// Answer with this status and no body.
    Status(u16),
    /// Ignore a Range header: send the whole file with 200.
    IgnoreRange,
    /// Send other bytes of the same length (a bad mirror).
    Corrupt,
    /// Wait this long after the headers before sending the body (a slow
    /// line: the download is still running meanwhile).
    Delay(std::time::Duration),
}

/// One request the server saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub path: String,
    pub range: Option<String>,
}

struct Shared {
    files: HashMap<String, Vec<u8>>,
    faults: Mutex<HashMap<String, VecDeque<Fault>>>,
    seen: Mutex<Vec<Seen>>,
    stop: AtomicBool,
}

pub struct FileServer {
    pub base: String,
    addr: std::net::SocketAddr,
    shared: Arc<Shared>,
}

impl FileServer {
    /// Serve `files` (name -> bytes) under `http://127.0.0.1:<port>/m/`.
    pub fn start(files: &[(&str, Vec<u8>)]) -> FileServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            files: files
                .iter()
                .map(|(n, b)| (format!("/m/{n}"), b.clone()))
                .collect(),
            faults: Mutex::new(HashMap::new()),
            seen: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                if s.stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(conn) = conn {
                    let s = s.clone();
                    thread::spawn(move || serve(conn, &s));
                }
            }
        });
        FileServer {
            base: format!("http://{addr}/m"),
            addr,
            shared,
        }
    }

    /// Queue a fault for the next request of `name`.
    pub fn fault(&self, name: &str, f: Fault) {
        self.shared
            .faults
            .lock()
            .unwrap()
            .entry(format!("/m/{name}"))
            .or_default()
            .push_back(f);
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.shared.seen.lock().unwrap().clone()
    }

    pub fn requests(&self) -> usize {
        self.seen().len()
    }
}

impl Drop for FileServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve(conn: TcpStream, s: &Shared) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    s.seen.lock().unwrap().push(Seen {
        path: path.clone(),
        range: range.clone(),
    });
    let fault = s
        .faults
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(|q| q.pop_front());
    let mut out = conn;
    let Some(file) = s.files.get(&path) else {
        let _ = out
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    if let Some(Fault::Status(code)) = fault {
        let _ = write!(
            out,
            "HTTP/1.1 {code} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    let mut body: Vec<u8> = file.clone();
    if fault == Some(Fault::Corrupt) {
        body.iter_mut().for_each(|b| *b ^= 0x5a);
    }
    let start = match (&range, &fault) {
        (Some(r), f) if *f != Some(Fault::IgnoreRange) => r
            .strip_prefix("bytes=")
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<usize>().ok()),
        _ => None,
    };
    let head = match start {
        Some(n) if n >= body.len() => {
            let _ = write!(
                out,
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            return;
        }
        Some(n) => {
            let h = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {n}-{}/{}\r\nConnection: close\r\n\r\n",
                body.len() - n,
                body.len() - 1,
                body.len()
            );
            body.drain(..n);
            h
        }
        None => format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
    };
    let _ = out.write_all(head.as_bytes());
    if let Some(Fault::Delay(d)) = &fault {
        let _ = out.flush();
        thread::sleep(*d);
    }
    match fault {
        Some(Fault::CutAfter(n)) => {
            let _ = out.write_all(&body[..n.min(body.len())]);
            let _ = out.flush();
        }
        _ => {
            let _ = out.write_all(&body);
        }
    }
}

/// One scripted answer of the `ScriptServer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering (a slow provider).
    pub delay: Option<std::time::Duration>,
}

impl Route {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Route {
        Route {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            delay: None,
        }
    }

    pub fn json(status: u16, body: &str) -> Route {
        Route::new(status, "application/json", body.as_bytes().to_vec())
    }

    pub fn wav(samples: &[i16], rate: u32) -> Route {
        let pcm = sonara_engine::PcmChunk {
            samples: samples.to_vec(),
            sample_rate: rate,
            channels: 1,
        };
        Route::new(200, "audio/wav", sonara_engine::wav::encode(&pcm))
    }

    pub fn header(mut self, k: &str, v: &str) -> Route {
        self.headers.push((k.into(), v.into()));
        self
    }

    pub fn delayed(mut self, d: std::time::Duration) -> Route {
        self.delay = Some(d);
        self
    }
}

/// One request the `ScriptServer` received.
#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    /// With the query.
    pub path: String,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == &name.to_ascii_lowercase())
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }
}

#[derive(Default)]
struct Script {
    fixed: HashMap<String, Route>,
    queued: HashMap<String, VecDeque<Route>>,
    seen: Vec<Captured>,
}

/// A local HTTP server that answers each path with scripted routes (one-off
/// answers queued first, then the path's fixed answer, else 404) and keeps
/// every request: a stand-in for a speech provider.
pub struct ScriptServer {
    /// `http://127.0.0.1:<port>`.
    pub base: String,
    addr: std::net::SocketAddr,
    script: Arc<Mutex<Script>>,
    stop: Arc<AtomicBool>,
}

impl ScriptServer {
    pub fn start() -> ScriptServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let script = Arc::new(Mutex::new(Script::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (script.clone(), stop.clone());
        thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(conn) = conn {
                    let s = s.clone();
                    thread::spawn(move || answer(conn, &s));
                }
            }
        });
        ScriptServer {
            base: format!("http://{addr}"),
            addr,
            script,
            stop,
        }
    }

    /// Answer every request for `path` (without the query) with `route`.
    pub fn on(&self, path: &str, route: Route) {
        self.script
            .lock()
            .unwrap()
            .fixed
            .insert(path.to_string(), route);
    }

    /// Answer the next request for `path` with `route` (before the fixed
    /// answer).
    pub fn queue(&self, path: &str, route: Route) {
        self.script
            .lock()
            .unwrap()
            .queued
            .entry(path.to_string())
            .or_default()
            .push_back(route);
    }

    pub fn requests(&self) -> Vec<Captured> {
        self.script.lock().unwrap().seen.clone()
    }

    /// Requests for `path` (without the query).
    pub fn count(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|c| c.path.split('?').next() == Some(path))
            .count()
    }
}

impl Drop for ScriptServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn answer(conn: TcpStream, script: &Mutex<Script>) {
    use std::io::Read;
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() || first.is_empty() {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let bare = path.split('?').next().unwrap_or("").to_string();
    let route = {
        let mut s = script.lock().unwrap();
        s.seen.push(Captured {
            method,
            path: path.clone(),
            headers,
            body,
        });
        s.queued
            .get_mut(&bare)
            .and_then(VecDeque::pop_front)
            .or_else(|| s.fixed.get(&bare).cloned())
    };
    let route = route.unwrap_or_else(|| Route::json(404, r#"{"detail": "Not Found"}"#));
    if let Some(d) = route.delay {
        thread::sleep(d);
    }
    let mut out = conn;
    let mut head = format!(
        "HTTP/1.1 {} Scripted\r\nContent-Length: {}\r\nConnection: close\r\n",
        route.status,
        route.body.len()
    );
    for (k, v) in &route.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&route.body);
    let _ = out.flush();
}

/// Lower-case hex SHA-256.
pub fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(b))
}

/// A NumPy `.npz` (stored zip) of float32 arrays, as `voices-v1.0.bin`:
/// voice `i` of `ids` has every style value equal to `i`.
pub fn voices_npz(ids: &[&str], rows: usize, dim: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let mut header =
            format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({rows}, 1, {dim}), }}");
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut data = b"\x93NUMPY\x01\x00".to_vec();
        data.extend((header.len() as u16).to_le_bytes());
        data.extend(header.as_bytes());
        for _ in 0..rows * dim {
            data.extend((i as f32).to_le_bytes());
        }
        let name = format!("{id}.npy");
        let offset = out.len() as u32;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(0u32.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(&data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(0u32.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let (at, len) = (out.len() as u32, central.len() as u32);
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((ids.len() as u16).to_le_bytes());
    out.extend((ids.len() as u16).to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend([0u8; 2]);
    out
}
