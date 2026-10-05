//! Request and reply over TCP JSON lines, as the CLI needs it: a `hello`,
//! then requests matched to their replies by `id`, with events kept for
//! `Conn::event`.
use crate::hello::Hello;
use crate::runtime::{connect, read_runtime, Runtime, PROBE};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Error, ErrorKind, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a request may wait for its reply.
pub const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Conn {
    reader: BufReader<TcpStream>,
    next_id: u64,
    events: Vec<Value>,
}

impl Conn {
    pub fn open(rt: &Runtime, timeout: Duration) -> std::io::Result<Conn> {
        let s = connect(rt, timeout)?;
        s.set_nodelay(true)?;
        Ok(Conn {
            reader: BufReader::new(s),
            next_id: 1,
            events: Vec::new(),
        })
    }

    /// Send `msg` and wait for its reply (events read meanwhile are kept).
    pub fn request(&mut self, msg: Value) -> std::io::Result<Value> {
        self.request_timeout(msg, TIMEOUT)
    }

    /// `request` with its own reply timeout (an engine test waits for the
    /// provider).
    pub fn request_timeout(&mut self, mut msg: Value, timeout: Duration) -> std::io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        msg["id"] = json!(id);
        let mut line = serde_json::to_vec(&msg)?;
        line.push(b'\n');
        self.reader.get_mut().write_all(&line)?;
        let end = Instant::now() + timeout;
        loop {
            let v = self.read(end)?;
            if v.get("id") == Some(&json!(id)) {
                return Ok(v);
            }
            if v.get("event").is_some() {
                self.events.push(v);
            }
        }
    }

    /// The next event (from a `subscribe`).
    pub fn event(&mut self, timeout: Duration) -> std::io::Result<Value> {
        if !self.events.is_empty() {
            return Ok(self.events.remove(0));
        }
        let end = Instant::now() + timeout;
        loop {
            let v = self.read(end)?;
            if v.get("event").is_some() {
                return Ok(v);
            }
        }
    }

    fn read(&mut self, end: Instant) -> std::io::Result<Value> {
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::new(ErrorKind::TimedOut, "no reply from the runtime"));
            }
            self.reader.get_ref().set_read_timeout(Some(left))?;
            let mut line = String::new();
            if self.reader.read_line(&mut line)? == 0 {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "the runtime closed the connection",
                ));
            }
            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                return Ok(v);
            }
        }
    }

    /// `get key`: the value, or `None` on an error reply.
    pub fn get(&mut self, key: &str) -> Option<Value> {
        let r = self.request(json!({"type": "get", "key": key})).ok()?;
        (r["ok"] == true).then(|| r["value"].clone())
    }
}

/// A connection to the runtime of `home` after a successful `hello`, the
/// runtime file's values and the `hello` reply; `None` when no runtime
/// answers.
pub fn attach(home: &Path, hello: &Hello) -> Option<(Runtime, Conn, Value)> {
    let rt = read_runtime(home)?;
    let mut c = Conn::open(&rt, PROBE).ok()?;
    let h = c.request(hello.message(&rt.token)).ok()?;
    (h["ok"] == true).then_some((rt, c, h))
}
