//! TCP JSON lines: one JSON object per line each way. The first message must
//! be `hello` with the token, else `E_AUTH` and the connection closes.
//! Replies and events share the connection; a reply has `ok`, an event has
//! `event`.
use crate::protocol::{After, Server, Session};
use crate::wire::{self, Code, Failure};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Longest accepted line (bytes, newline included).
pub const MAX_LINE: usize = 2 << 20;

/// How long a new connection may wait before a successful `hello`; then it
/// gets `E_AUTH` and is closed.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn serve(listener: TcpListener, server: Arc<Server>) {
    serve_with(listener, server, HELLO_TIMEOUT).await
}

pub async fn serve_with(listener: TcpListener, server: Arc<Server>, hello_timeout: Duration) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let server = server.clone();
                tokio::spawn(async move {
                    let _ = connection(stream, server, hello_timeout).await;
                });
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

enum Line {
    Text(String),
    /// Too long, or not UTF-8: answered, then the connection closes.
    Bad(Failure),
}

/// Reads lines on its own task, so a partly read line is never lost when an
/// event is written meanwhile.
async fn read_lines(stream: tokio::net::tcp::OwnedReadHalf, tx: mpsc::Sender<Line>) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut buf = Vec::new();
        let n = match (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        let line = if buf.len() > MAX_LINE {
            Line::Bad(Failure::new(Code::BadRequest, "line too long"))
        } else {
            match String::from_utf8(buf) {
                Ok(s) => Line::Text(s),
                Err(_) => Line::Bad(Failure::new(Code::BadRequest, "not UTF-8")),
            }
        };
        let stop = matches!(line, Line::Bad(_));
        if tx.send(line).await.is_err() || stop {
            return;
        }
    }
}

async fn send(w: &mut tokio::net::tcp::OwnedWriteHalf, v: &str) -> std::io::Result<()> {
    let mut line = String::with_capacity(v.len() + 1);
    line.push_str(v);
    line.push('\n');
    w.write_all(line.as_bytes()).await
}

async fn connection(
    stream: TcpStream,
    server: Arc<Server>,
    hello_timeout: Duration,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let (rd, mut wr) = stream.into_split();
    let (tx, mut lines) = mpsc::channel(16);
    let reader_task = tokio::spawn(read_lines(rd, tx));
    let mut session = Session::tcp();
    let mut events: Option<mpsc::Receiver<crate::events::WireEvent>> = None;
    let mut client = None;
    // Keeps the `system` extension armed while this connection lives.
    let mut system_hold = None;
    let hello_deadline = tokio::time::sleep(hello_timeout);
    tokio::pin!(hello_deadline);
    let result = loop {
        let next_event = async {
            match events.as_mut() {
                Some(rx) => rx.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            line = lines.recv() => {
                let text = match line {
                    None => break Ok(()),
                    Some(Line::Bad(f)) => {
                        let _ = send(&mut wr, &wire::error_reply(None, &f).to_string()).await;
                        break Ok(());
                    }
                    Some(Line::Text(t)) => t,
                };
                if text.trim().is_empty() {
                    continue;
                }
                server.lifetime().touch();
                let request: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) if session.authed => {
                        let f = Failure::new(Code::BadRequest, format!("invalid JSON: {e}"));
                        if let Err(e) = send(&mut wr, &wire::error_reply(None, &f).to_string()).await {
                            break Err(e);
                        }
                        continue;
                    }
                    // Before hello, garbage is an authentication failure.
                    Err(_) => Value::Null,
                };
                let srv = server.clone();
                let s = session;
                let (outcome, s) = match tokio::task::spawn_blocking(move || {
                    let mut s = s;
                    let o = srv.handle(&mut s, &request);
                    (o, s)
                })
                .await
                {
                    Ok(r) => r,
                    Err(_) => break Ok(()),
                };
                session = s;
                if session.authed && client.is_none() {
                    client = Some(server.lifetime().client());
                }
                if session.system && system_hold.is_none() {
                    let srv = server.clone();
                    system_hold = tokio::task::spawn_blocking(move || srv.hold_system())
                        .await
                        .ok()
                        .flatten();
                }
                if let Err(e) = send(&mut wr, &outcome.reply.to_string()).await {
                    break Err(e);
                }
                match outcome.after {
                    After::Nothing => {}
                    After::Close => break Ok(()),
                    After::Subscribe(rx) => events = Some(rx),
                    After::Exit => {
                        let _ = wr.flush().await;
                        server.exit_for_takeover();
                        break Ok(());
                    }
                }
            }
            _ = &mut hello_deadline, if !session.authed => {
                let f = Failure::new(Code::Auth, "no hello with the token in time");
                let _ = send(&mut wr, &wire::error_reply(None, &f).to_string()).await;
                break Ok(());
            }
            ev = next_event => {
                match ev {
                    Some(ev) => {
                        if let Err(e) = send(&mut wr, &ev.json).await {
                            break Err(e);
                        }
                    }
                    None => events = None,
                }
            }
        }
    };
    reader_task.abort();
    let _ = wr.shutdown().await;
    if let Some(hold) = system_hold {
        // The last client that needed `system` left: other apps are
        // restored and the hotkeys released (joins their threads).
        let _ = tokio::task::spawn_blocking(move || drop(hold)).await;
    }
    drop(client);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifetime::Lifetime;
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, ReaderHandle, Registry};
    use std::time::Duration;

    fn server() -> Arc<Server> {
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        let life = Lifetime::new(Duration::from_secs(30), false);
        Arc::new(Server::new(reader, "secret".into(), life))
    }

    #[tokio::test]
    async fn a_connection_without_hello_is_closed_with_e_auth() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_with(listener, server(), Duration::from_millis(200)));
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut lines = BufReader::new(stream);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), lines.read_line(&mut line))
            .await
            .expect("the silent connection was not closed")
            .unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["code"], "E_AUTH");
        line.clear();
        let n = tokio::time::timeout(Duration::from_secs(5), lines.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0, "the connection closes after E_AUTH");
    }
}
