//! Fire and forget, as a hook needs it: `hello` and a batch of messages on
//! one connection, so they apply in order, with the runtime started when
//! none answers. Never blocks past the deadline it is given.
use crate::hello::Hello;
use crate::home::stopped;
use crate::runtime::{connect, read_runtime, start_runtime, Runtime, PROBE};
use serde_json::Value;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

/// How often `deliver` looks for the new runtime's `runtime.json`.
const POLL: Duration = Duration::from_millis(25);

/// Send `hello` and `msgs` on one connection, then read the replies so the
/// runtime has applied them all before the connection closes. Returns the
/// replies (hello first).
pub fn send(
    rt: &Runtime,
    hello: &Hello,
    msgs: &[Value],
    timeout: Duration,
) -> std::io::Result<Vec<Value>> {
    let s = connect(rt, timeout)?;
    send_on(s, rt, hello, msgs, timeout)
}

/// `send` on a connection already made. An error means the batch was not
/// written; once it was, the replies that came before the connection
/// closed or `timeout` ran out are returned (a slow runtime still applies
/// the batch: it must not be sent again).
pub fn send_on(
    s: TcpStream,
    rt: &Runtime,
    hello: &Hello,
    msgs: &[Value],
    timeout: Duration,
) -> std::io::Result<Vec<Value>> {
    exchange(s, rt, hello, msgs, timeout).map(|e| e.replies)
}

/// What `send_on` got back for a batch it wrote.
struct Exchange {
    /// The replies, hello first.
    replies: Vec<Value>,
    /// The wait for them ran out (the connection did not close).
    timed_out: bool,
}

fn exchange(
    mut s: TcpStream,
    rt: &Runtime,
    hello: &Hello,
    msgs: &[Value],
    timeout: Duration,
) -> std::io::Result<Exchange> {
    s.set_nodelay(true)?;
    let hello = hello.message(&rt.token);
    let mut batch = Vec::new();
    for m in std::iter::once(&hello).chain(msgs) {
        serde_json::to_writer(&mut batch, m)?;
        batch.push(b'\n');
    }
    s.write_all(&batch)?;
    s.flush()?;
    let end = Instant::now() + timeout;
    let mut replies = Vec::new();
    let mut timed_out = false;
    let mut r = BufReader::new(s);
    while replies.len() < msgs.len() + 1 {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            timed_out = true;
            break;
        }
        if r.get_ref().set_read_timeout(Some(left)).is_err() {
            break;
        }
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            // The read timeout: `TimedOut` on Windows, `WouldBlock` on Unix.
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                timed_out = true;
                break;
            }
            Err(_) => break,
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v.get("ok").is_some() {
                replies.push(v);
            }
        }
    }
    Ok(Exchange { replies, timed_out })
}

/// What `deliver` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Sent to the runtime that was running.
    Sent,
    /// Started the runtime, then sent.
    Started,
    /// Nothing reached a runtime (none running and none started in time,
    /// or the start is off).
    Dropped,
}

/// Whether the replies of `send_on` start with an ok reply to `hello`.
fn hello_ok(replies: &[Value]) -> bool {
    replies
        .first()
        .and_then(|v| v.get("ok"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Whether the wait after a start should try `rt` (the `runtime.json` now
/// in the home): a runtime other than the `stale` one that did not answer,
/// or the stale one again once the start has exited (a second runtime
/// exits at once on the single-instance mutex, so the one named is alive,
/// only slow to answer the first probe).
pub fn worth_trying(rt: &Runtime, stale: Option<u64>, start_exited: bool) -> bool {
    start_exited || rt.pid.is_none() || rt.pid != stale
}

/// Send `msgs` (after `hello`) to the runtime of `home`, starting `exe`
/// (with `args`) when none answers, `exe` is given and the home is not
/// `stopped`; the start and the wait for it end by `deadline`. Never
/// blocks past the deadline for the start, nor past `timeout` for the
/// replies. A batch is written at most once to a runtime: one that got it
/// and did not count as delivered (`delivered`) is not tried again.
pub fn deliver(
    home: &Path,
    hello: &Hello,
    msgs: &[Value],
    exe: Option<&Path>,
    args: &[String],
    deadline: Instant,
    timeout: Duration,
) -> Delivery {
    deliver_with(home, hello, msgs, exe, deadline, timeout, &|exe| {
        start_runtime(exe, args)
    })
}

/// `connect` checked that the port is held by the process `runtime.json`
/// names (#56): it does when the file has a pid and the platform can tell
/// who holds a port (`runtime.rs` `OWNER_CHECK`).
fn owner_checked(rt: &Runtime) -> bool {
    rt.pid.is_some() && cfg!(any(windows, target_os = "macos"))
}

/// The runtime has the batch written to it: it answered hello ok (later
/// replies may still be out), or said nothing before the wait ran out
/// while `connect` had checked that it holds the port (a busy runtime, not
/// a squatter). A listener that answers anything else, or closes without
/// an ok hello, is not the runtime.
fn delivered(rt: &Runtime, e: &Exchange) -> bool {
    hello_ok(&e.replies) || (e.replies.is_empty() && e.timed_out && owner_checked(rt))
}

/// `deliver`, with `start` starting the runtime (the tests: a stand-in).
fn deliver_with(
    home: &Path,
    hello: &Hello,
    msgs: &[Value],
    exe: Option<&Path>,
    deadline: Instant,
    timeout: Duration,
    start: &dyn Fn(&Path) -> std::io::Result<Child>,
) -> Delivery {
    let before = read_runtime(home);
    let mut written: Option<Runtime> = None;
    if let Some(rt) = &before {
        // A crashed runtime leaves runtime.json behind, and another program
        // may hold its port by now: only an ok reply to hello is the runtime.
        if let Ok(s) = connect(rt, PROBE) {
            if let Ok(e) = exchange(s, rt, hello, msgs, timeout) {
                if delivered(rt, &e) {
                    return Delivery::Sent;
                }
                written = Some(rt.clone());
            }
        }
    }
    let Some(exe) = exe.filter(|_| !stopped(home)) else {
        return Delivery::Dropped;
    };
    if Instant::now() >= deadline {
        // No time left to start one and wait for it.
        return Delivery::Dropped;
    }
    let Ok(mut child) = start(exe) else {
        return Delivery::Dropped;
    };
    let stale = before.and_then(|r| r.pid);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        let exited = matches!(child.try_wait(), Ok(Some(_)));
        let Some(rt) = read_runtime(home)
            .filter(|r| worth_trying(r, stale, exited) && written.as_ref() != Some(r))
        else {
            continue;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(s) = connect(&rt, left.min(PROBE)) {
            let _ = exchange(s, &rt, hello, msgs, timeout);
            return Delivery::Started;
        }
    }
    Delivery::Dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::STOPPED;
    use serde_json::json;

    const HELLO: Hello = Hello {
        name: "test",
        version: "0",
        extensions: &[],
        keep_alive: false,
    };

    #[test]
    fn without_a_runtime_or_an_exe_nothing_is_delivered_at_once() {
        let dir = std::env::temp_dir().join(format!("sonara-client-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Instant::now();
        let d = deliver(
            &dir,
            &HELLO,
            &[json!({"type": "stream"})],
            None,
            &[],
            t + Duration::from_secs(1),
            Duration::from_secs(2),
        );
        assert_eq!(d, Delivery::Dropped);
        assert!(t.elapsed() < Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wait_takes_a_new_runtime_or_the_old_one_once_the_start_is_gone() {
        let rt = |pid| Runtime {
            port: 1,
            token: "t".into(),
            pid,
        };
        assert!(worth_trying(&rt(Some(2)), Some(1), false), "a new runtime");
        assert!(worth_trying(&rt(None), Some(1), false), "no pid to compare");
        assert!(
            !worth_trying(&rt(Some(1)), Some(1), false),
            "the stale one while the start may still replace it"
        );
        assert!(
            worth_trying(&rt(Some(1)), Some(1), true),
            "the start exited (the mutex: the old runtime is alive), so retry it"
        );
    }

    // #56: a stale runtime.json whose port another process now holds gets
    // nothing: not the hello with the token, not the messages.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn nothing_is_sent_to_a_process_squatting_the_port_of_a_dead_runtime() {
        use crate::home::RUNTIME_FILE;
        use std::io::Read;
        use std::net::TcpListener;
        let dir = std::env::temp_dir().join(format!("sonara-client-squat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let dead = u64::from(std::process::id()) + 4;
        std::fs::write(
            dir.join(RUNTIME_FILE),
            json!({"pid": dead, "port": port, "token": "secret"}).to_string(),
        )
        .unwrap();
        let squatter = std::thread::spawn(move || {
            l.set_nonblocking(false).unwrap();
            let Ok((mut s, _)) = l.accept() else {
                return Vec::new();
            };
            s.set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let mut got = Vec::new();
            let _ = s.read_to_end(&mut got);
            got
        });
        let t = Instant::now();
        let d = deliver(
            &dir,
            &HELLO,
            &[json!({"type": "stream", "text": "the turn"})],
            None,
            &[],
            t + Duration::from_secs(1),
            Duration::from_secs(2),
        );
        assert_eq!(d, Delivery::Dropped);
        let got = squatter.join().unwrap();
        assert!(got.is_empty(), "{}", String::from_utf8_lossy(&got));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_home_is_not_started() {
        let dir = std::env::temp_dir().join(format!("sonara-client-stop-d-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STOPPED), b"").unwrap();
        // A start is not even tried: a missing exe would be Dropped at once
        // too, so use an exe path that would fail loudly if spawned.
        let t = Instant::now();
        let d = deliver(
            &dir,
            &HELLO,
            &[json!({"type": "stream"})],
            Some(&dir.join("no-such-sonarad.exe")),
            &[],
            t + Duration::from_secs(1),
            Duration::from_secs(2),
        );
        assert_eq!(d, Delivery::Dropped);
        assert!(t.elapsed() < Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_listener_that_does_not_answer_hello_ok_is_not_the_runtime() {
        use std::net::TcpListener;
        let dir =
            std::env::temp_dir().join(format!("sonara-client-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Another program on the port a crashed runtime left in runtime.json.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::fs::write(
            dir.join(crate::home::RUNTIME_FILE),
            json!({"port": port, "token": "old", "pid": 1}).to_string(),
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            for reply in [
                &b"{\"ok\":false}\n"[..],
                b"HTTP/1.1 400 Bad Request\r\n\r\n",
            ] {
                let (s, _) = l.accept().unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                (&s).write_all(reply).unwrap();
            }
        });
        for _ in 0..2 {
            let d = deliver(
                &dir,
                &HELLO,
                &[json!({"type": "stream"})],
                None,
                &[],
                Instant::now() + Duration::from_secs(1),
                Duration::from_secs(2),
            );
            assert_eq!(d, Delivery::Dropped, "not counted as sent");
        }
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A runtime (this process, so `connect`'s owner check passes) that
    /// reads hello and one message, answers hello ok when `answer`, then
    /// stalls. Returns the lines it read and how many more connections came.
    fn stalling_runtime(dir: &Path, answer: bool) -> std::thread::JoinHandle<(Vec<String>, usize)> {
        use std::net::TcpListener;
        std::fs::create_dir_all(dir).unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::fs::write(
            dir.join(crate::home::RUNTIME_FILE),
            json!({"pid": std::process::id(), "port": port, "token": "tok"}).to_string(),
        )
        .unwrap();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut lines = Vec::new();
            for _ in 0..2 {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                lines.push(line);
            }
            if answer {
                (&s).write_all(b"{\"ok\":true}\n").unwrap();
            }
            l.set_nonblocking(true).unwrap();
            let mut more = 0;
            let end = Instant::now() + Duration::from_millis(1200);
            while Instant::now() < end {
                if l.accept().is_ok() {
                    more += 1;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(s);
            (lines, more)
        })
    }

    fn no_start(called: &std::cell::Cell<bool>) -> impl Fn(&Path) -> std::io::Result<Child> + '_ {
        |_| {
            called.set(true);
            Err(std::io::Error::other("no start in tests"))
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_runtime_that_answers_hello_then_stalls_got_the_batch_once() {
        // Audit 2026-11-23: a reply that timed out made the batch count as
        // not sent, so a runtime was started and the batch could go again.
        let dir = std::env::temp_dir().join(format!("sonara-client-slow-{}", std::process::id()));
        let server = stalling_runtime(&dir, true);
        let called = std::cell::Cell::new(false);
        let d = deliver_with(
            &dir,
            &HELLO,
            &[json!({"type": "stream", "delta": "the turn"})],
            Some(&dir.join("sonarad.exe")),
            Instant::now() + Duration::from_secs(1),
            Duration::from_millis(300),
            &no_start(&called),
        );
        assert_eq!(d, Delivery::Sent);
        assert!(!called.get(), "no runtime started");
        let (lines, more) = server.join().unwrap();
        assert!(
            lines[0].contains("hello") && lines[1].contains("the turn"),
            "{lines:?}"
        );
        assert_eq!(more, 0, "the batch went once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn a_runtime_that_holds_its_port_and_says_nothing_got_the_batch_once() {
        let dir = std::env::temp_dir().join(format!("sonara-client-mute-{}", std::process::id()));
        let server = stalling_runtime(&dir, false);
        let called = std::cell::Cell::new(false);
        let d = deliver_with(
            &dir,
            &HELLO,
            &[json!({"type": "stream", "delta": "the turn"})],
            Some(&dir.join("sonarad.exe")),
            Instant::now() + Duration::from_secs(1),
            Duration::from_millis(300),
            &no_start(&called),
        );
        assert_eq!(d, Delivery::Sent);
        assert!(!called.get(), "no runtime started");
        let (_, more) = server.join().unwrap();
        assert_eq!(more, 0, "the batch went once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_runtime_is_started_once_the_deadline_passed() {
        let dir = std::env::temp_dir().join(format!("sonara-client-late-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let called = std::cell::Cell::new(false);
        let d = deliver_with(
            &dir,
            &HELLO,
            &[json!({"type": "stream"})],
            Some(&dir.join("sonarad.exe")),
            Instant::now(),
            Duration::from_millis(300),
            &no_start(&called),
        );
        assert_eq!(d, Delivery::Dropped);
        assert!(!called.get(), "no start after the deadline");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_batch_goes_after_hello_on_one_connection_and_the_replies_come_back() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let rt = Runtime {
            port: l.local_addr().unwrap().port(),
            token: "tok".into(),
            pid: None,
        };
        let server = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut got = Vec::new();
            for _ in 0..3 {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                got.push(serde_json::from_str::<Value>(&line).unwrap());
                (&s).write_all(b"{\"ok\":true}\n").unwrap();
            }
            got
        });
        let msgs = [json!({"type": "focus"}), json!({"type": "turn_end"})];
        let replies = send(&rt, &HELLO, &msgs, Duration::from_secs(2)).unwrap();
        assert_eq!(replies.len(), 3);
        let got = server.join().unwrap();
        assert_eq!(got[0]["type"], "hello");
        assert_eq!(got[0]["token"], "tok");
        assert_eq!(got[1]["type"], "focus");
        assert_eq!(got[2]["type"], "turn_end");
    }
}
