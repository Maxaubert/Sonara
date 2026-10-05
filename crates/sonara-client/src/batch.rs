//! Fire and forget, as a hook needs it: `hello` and a batch of messages on
//! one connection, so they apply in order, with the runtime started when
//! none answers. Never blocks past the deadline it is given.
use crate::hello::Hello;
use crate::home::stopped;
use crate::runtime::{connect, read_runtime, start_runtime, Runtime, PROBE};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
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

/// `send` on a connection already made.
pub fn send_on(
    mut s: TcpStream,
    rt: &Runtime,
    hello: &Hello,
    msgs: &[Value],
    timeout: Duration,
) -> std::io::Result<Vec<Value>> {
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
    let mut r = BufReader::new(s);
    while replies.len() < msgs.len() + 1 {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        r.get_ref().set_read_timeout(Some(left))?;
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v.get("ok").is_some() {
                replies.push(v);
            }
        }
    }
    Ok(replies)
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
/// replies.
pub fn deliver(
    home: &Path,
    hello: &Hello,
    msgs: &[Value],
    exe: Option<&Path>,
    args: &[String],
    deadline: Instant,
    timeout: Duration,
) -> Delivery {
    let before = read_runtime(home);
    if let Some(rt) = &before {
        if let Ok(s) = connect(rt, PROBE) {
            let _ = send_on(s, rt, hello, msgs, timeout);
            return Delivery::Sent;
        }
    }
    let Some(exe) = exe.filter(|_| !stopped(home)) else {
        return Delivery::Dropped;
    };
    let Ok(mut child) = start_runtime(exe, args) else {
        return Delivery::Dropped;
    };
    let stale = before.and_then(|r| r.pid);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        let exited = matches!(child.try_wait(), Ok(Some(_)));
        let Some(rt) = read_runtime(home).filter(|r| worth_trying(r, stale, exited)) else {
            continue;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(s) = connect(&rt, left.min(PROBE)) {
            let _ = send_on(s, &rt, hello, msgs, timeout);
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
