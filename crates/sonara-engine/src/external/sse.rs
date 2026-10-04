//! One request whose answer is server-sent events (#235, Gemini's
//! `streamGenerateContent?alt=sse`), read on its own thread as it arrives:
//! each event's `data` goes to the receiver at once, so the first audio can
//! play while the rest is still being made. The thread ends when the
//! answer ends, the agent's timeouts end it, or the receiver is dropped
//! (the wait for it was cancelled or gave up).
use super::adapter::{head, send, transport, HttpReply, HttpRequest, MAX_ERROR_BODY};
use super::error::ExtError;
use crate::Reason;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::{channel, Receiver, Sender};

/// The most a streamed answer may hold (base64 audio of a long chunk).
pub const MAX_STREAM_BODY: u64 = 128 * 1024 * 1024;

/// What the request thread saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The `data` of one event (several `data:` lines joined with `\n`).
    Data(String),
    /// The answer ended.
    End,
    /// A non-2xx answer, its error body read (for `Adapter::map_error`).
    Refused(HttpReply),
    /// The request or the body failed (`network`, `timeout`, `format`).
    Failed(ExtError),
}

/// Start `req` on its own thread; its events come on the receiver.
pub fn start(agent: ureq::Agent, req: HttpRequest, host: String) -> Receiver<Event> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("sonara-external-stream".into())
        .spawn({
            let tx = tx.clone();
            move || run(&agent, &req, &host, &tx)
        });
    if spawned.is_err() {
        let _ = tx.send(Event::Failed(ExtError::new(
            Reason::Network,
            "cannot start the request thread",
        )));
    }
    rx
}

fn run(agent: &ureq::Agent, req: &HttpRequest, host: &str, tx: &Sender<Event>) {
    let mut resp = match send(agent, req) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(Event::Failed(transport(e, host)));
            return;
        }
    };
    let (status, content_type, retry_after) = head(&resp);
    if !(200..300).contains(&status) {
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_ERROR_BODY)
            .read_to_vec()
            .unwrap_or_default();
        let _ = tx.send(Event::Refused(HttpReply {
            status,
            content_type,
            retry_after,
            body,
        }));
        return;
    }
    let reader = resp
        .into_body()
        .into_with_config()
        .limit(MAX_STREAM_BODY)
        .reader();
    let failed = |e: std::io::Error| {
        // ureq reports its own failures (a timeout, the size limit) as
        // the source of the io error.
        let inner = e
            .get_ref()
            .and_then(|i| i.downcast_ref::<ureq::Error>())
            .map(|u| match u {
                ureq::Error::Timeout(_) => Reason::Timeout,
                ureq::Error::BodyExceedsLimit(_) => Reason::Format,
                _ => Reason::Network,
            });
        let reason = inner.unwrap_or(if e.kind() == std::io::ErrorKind::TimedOut {
            Reason::Timeout
        } else {
            Reason::Network
        });
        let message = match reason {
            Reason::Timeout => format!("{host} did not finish the answer in time"),
            Reason::Format => {
                format!("the answer of {host} is larger than {MAX_STREAM_BODY} bytes")
            }
            _ => format!("the answer of {host} broke off: {e}"),
        };
        ExtError::new(reason, message)
    };
    let mut lines = BufReader::new(reader);
    let mut data: Option<String> = None;
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(Event::Failed(failed(e)));
                return;
            }
        }
        let l = line.trim_end_matches(['\r', '\n']);
        if l.is_empty() {
            // The end of one event.
            if let Some(d) = data.take() {
                if tx.send(Event::Data(d)).is_err() {
                    return;
                }
            }
            continue;
        }
        if let Some(v) = l.strip_prefix("data:") {
            let v = v.strip_prefix(' ').unwrap_or(v);
            match &mut data {
                Some(d) => {
                    d.push('\n');
                    d.push_str(v);
                }
                None => data = Some(v.to_string()),
            }
        }
        // `event:`, `id:`, `retry:` and comments (`:`) carry nothing here.
    }
    if let Some(d) = data.take() {
        if tx.send(Event::Data(d)).is_err() {
            return;
        }
    }
    let _ = tx.send(Event::End);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// A one-shot server: answers the first request with `head` and then
    /// `chunks`, each after its delay.
    fn serve(head: &'static str, chunks: Vec<(u64, &'static str)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            s.write_all(head.as_bytes()).unwrap();
            for (ms, c) in chunks {
                std::thread::sleep(Duration::from_millis(ms));
                if s.write_all(c.as_bytes()).is_err() {
                    return;
                }
                let _ = s.flush();
            }
        });
        format!("http://{addr}/s")
    }

    fn agent() -> ureq::Agent {
        crate::http::provider_agent(
            crate::http::Timeouts {
                connect: Duration::from_secs(2),
                recv_response: Duration::from_secs(5),
                recv_body: Duration::from_secs(5),
            },
            true,
        )
    }

    const SSE: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

    #[test]
    fn events_arrive_one_by_one_as_they_are_sent() {
        let url = serve(
            SSE,
            vec![
                (0, "data: {\"a\":1}\r\n\r\n"),
                (300, ": a comment\n\nevent: x\ndata: two\ndata: lines\n\n"),
                (0, "data: last"),
            ],
        );
        let rx = start(agent(), HttpRequest::get(url), "h".into());
        let first = rx.recv_timeout(Duration::from_millis(250));
        assert_eq!(
            first,
            Ok(Event::Data("{\"a\":1}".into())),
            "before the rest"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::Data("two\nlines".into()))
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Event::Data("last".into()))
        );
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::End));
    }

    #[test]
    fn a_refusal_carries_its_status_and_body() {
        let url = serve(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"error\":\"x\"}",
            vec![],
        );
        let rx = start(agent(), HttpRequest::get(url), "h".into());
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Refused(r) => {
                assert_eq!(r.status, 404);
                assert_eq!(r.body, b"{\"error\":\"x\"}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_server_is_a_transport_failure() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let rx = start(
            agent(),
            HttpRequest::get(format!("http://{addr}/s")),
            "h".into(),
        );
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            // Windows retries a refused connect until the connect timeout.
            Event::Failed(e) => assert!(
                matches!(e.reason, Reason::Network | Reason::Timeout),
                "{e:?}"
            ),
            other => panic!("{other:?}"),
        }
    }
}
