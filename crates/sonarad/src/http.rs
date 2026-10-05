//! HTTP/1.1: `POST /v1/<type>` with a JSON body and `GET /v1/events` as
//! Server-Sent Events, both with `Authorization: Bearer <token>`; and
//! `GET /settings?token=<token>`, the settings page of the `system`
//! extension (`settings_page`).
//!
//! The body of a POST is the request without `type` (the path gives it); an
//! empty body is `{}`. The reply body is the same JSON as on TCP; the status
//! is 200 for `ok: true` and follows the error code otherwise
//! (`wire::Code::http_status`).
use crate::events::{EventSet, WireEvent};
use crate::lifetime::ClientGuard;
use crate::protocol::{After, Server, Session};
use crate::wire::{self, Code, Failure};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{HeaderName, HeaderValue, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Map, Value};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Largest accepted request body.
pub const MAX_BODY: usize = 1 << 20;
/// An SSE comment is sent this often so dead connections are noticed.
pub const PING: Duration = Duration::from_secs(15);

type BoxBody = http_body_util::combinators::UnsyncBoxBody<Bytes, Infallible>;

/// After the exit is decided, connections keep being accepted (and answered
/// `E_BUSY`) until none arrived for this long, then the listener closes.
const DRAIN_QUIET: Duration = Duration::from_millis(50);
/// The drain never takes longer than this.
const DRAIN_MAX: Duration = Duration::from_secs(1);

/// Serves until the exit is decided, then answers the connections that are
/// already waiting (`E_BUSY`) and returns, closing the listener. `main`
/// waits for this and for `Lifetime::requests_done` before the process
/// ends, so no request loses its connection without a reply (#247).
pub async fn serve(listener: TcpListener, server: Arc<Server>) {
    let life = server.lifetime().clone();
    loop {
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = life.wait_exit() => break,
        };
        match accepted {
            Ok((stream, _)) => connection(stream, &server),
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let end = tokio::time::Instant::now() + DRAIN_MAX;
    loop {
        let quiet = (tokio::time::Instant::now() + DRAIN_QUIET).min(end);
        match tokio::time::timeout_at(quiet, listener.accept()).await {
            Ok(Ok((stream, _))) => connection(stream, &server),
            Ok(Err(_)) => continue,
            Err(_) => return,
        }
    }
}

/// One connection on its own task. It counts as a pending request until
/// its first reply (so one accepted just before the exit is answered), and
/// so does each request on it.
fn connection(stream: tokio::net::TcpStream, server: &Arc<Server>) {
    let server = server.clone();
    let first = Arc::new(std::sync::Mutex::new(Some(server.lifetime().request())));
    tokio::spawn(async move {
        let service = service_fn(move |req| {
            let server = server.clone();
            let first = first.clone();
            async move {
                let _pending = server.lifetime().request();
                let reply = route(req, server).await;
                first.lock().unwrap_or_else(|e| e.into_inner()).take();
                Ok::<_, Infallible>(reply)
            }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .await;
    });
}

fn json_response(status: u16, v: &Value) -> Response<BoxBody> {
    let mut r = Response::new(Full::new(Bytes::from(v.to_string())).boxed_unsync());
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

fn failure(f: Failure) -> Response<BoxBody> {
    json_response(f.code.http_status(), &wire::error_reply(None, &f))
}

fn bearer_ok(req: &Request<Incoming>, server: &Server) -> bool {
    req.headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| server.token_ok(t.trim()))
        .unwrap_or(false)
}

/// `GET /settings`: the page with the token injected, or a short text
/// error (`settings_page` has the rules).
fn settings(req: &Request<Incoming>, server: &Server) -> Response<BoxBody> {
    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok());
    let page = match server.system() {
        Some(s) => s.settings_page(host, req.uri().query()),
        None => Err((404, "this runtime has no settings page")),
    };
    match page {
        Ok(html) => {
            let mut r = Response::new(Full::new(Bytes::from(html)).boxed_unsync());
            for (k, v) in crate::settings_page::HEADERS {
                r.headers_mut()
                    .insert(HeaderName::from_static(k), HeaderValue::from_static(v));
            }
            r
        }
        Err((status, message)) => {
            let mut r = Response::new(Full::new(Bytes::from(message)).boxed_unsync());
            *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::NOT_FOUND);
            r.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            r
        }
    }
}

async fn route(req: Request<Incoming>, server: Arc<Server>) -> Response<BoxBody> {
    let path = req.uri().path().to_string();
    if path == "/settings" && req.method() == Method::GET {
        server.lifetime().touch();
        let srv = server.clone();
        return settings(&req, &srv);
    }
    let Some(kind) = path.strip_prefix("/v1/").map(str::to_string) else {
        return failure(Failure::new(Code::NotFound, format!("no route {path}")));
    };
    if !bearer_ok(&req, &server) {
        return failure(Failure::new(
            Code::Auth,
            "missing or wrong 'Authorization: Bearer <token>'",
        ));
    }
    server.lifetime().touch();
    if server.lifetime().exit_requested().is_some() {
        // The reader may already be shut down: say so instead of failing
        // with an engine error (#194).
        return failure(Failure::new(Code::Busy, "this runtime is exiting"));
    }
    match (req.method(), kind.as_str()) {
        (&Method::GET, "events") => events(req, server).await,
        (&Method::POST, _) if !kind.is_empty() && !kind.contains('/') => {
            post(req, kind, server).await
        }
        _ => failure(Failure::new(
            Code::BadRequest,
            "use POST /v1/<type> or GET /v1/events",
        )),
    }
}

async fn post(req: Request<Incoming>, kind: String, server: Arc<Server>) -> Response<BoxBody> {
    let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return failure(Failure::new(
                Code::BadRequest,
                "body unreadable or too large",
            ))
        }
    };
    let mut map = if body.iter().all(u8::is_ascii_whitespace) {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                return failure(Failure::new(
                    Code::BadRequest,
                    "the body must be a JSON object",
                ))
            }
            Err(e) => return failure(Failure::new(Code::BadRequest, format!("invalid JSON: {e}"))),
        }
    };
    map.insert("type".into(), Value::String(kind));
    let request = Value::Object(map);
    let srv = server.clone();
    let outcome = match tokio::task::spawn_blocking(move || {
        let mut session = Session::http();
        srv.handle(&mut session, &request)
    })
    .await
    {
        Ok(o) => o,
        Err(_) => return failure(Failure::new(Code::Engine, "the request failed")),
    };
    let status = outcome.code.map(|c| c.http_status()).unwrap_or(200);
    if let After::Exit = outcome.after {
        // Let the reply go out before the process ends.
        let srv = server.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            srv.exit_for_takeover();
        });
    }
    json_response(status, &outcome.reply)
}

/// `?events=state,items,log` (default: all three).
fn event_set(query: Option<&str>) -> Result<EventSet, Failure> {
    let list = query
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "events")
        .map(|(_, v)| v.replace("%2C", ",").replace("%2c", ","));
    match list {
        None => Ok(EventSet::ALL),
        Some(v) => EventSet::parse(v.split(',').filter(|s| !s.is_empty()))
            .map_err(|n| Failure::new(Code::Unsupported, format!("unknown event stream '{n}'"))),
    }
}

async fn events(req: Request<Incoming>, server: Arc<Server>) -> Response<BoxBody> {
    let set = match event_set(req.uri().query()) {
        Ok(s) => s,
        Err(f) => return failure(f),
    };
    // Count the stream as a client before subscribing, so the idle exit
    // cannot be decided in between (#194).
    let client = server.lifetime().client();
    let srv = server.clone();
    let rx = match tokio::task::spawn_blocking(move || srv.events(set)).await {
        Ok(Ok(rx)) => rx,
        Ok(Err(f)) => return failure(f),
        Err(_) => return failure(Failure::new(Code::Engine, "cannot subscribe")),
    };
    let body = SseBody {
        rx,
        ping: tokio::time::interval_at(tokio::time::Instant::now() + PING, PING),
        _client: client,
        opened: false,
    };
    let mut r = Response::new(body.boxed_unsync());
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    r.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

/// The SSE stream of one client; counts as a client while open.
struct SseBody {
    rx: mpsc::Receiver<WireEvent>,
    ping: tokio::time::Interval,
    _client: ClientGuard,
    /// The opening comment was sent (flushes the headers at once).
    opened: bool,
}

pub fn sse_frame(e: &WireEvent) -> String {
    format!("event: {}\ndata: {}\n\n", e.name, e.json)
}

impl Body for SseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if !self.opened {
            self.opened = true;
            return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b": sonarad\n\n")))));
        }
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(e)) => {
                return Poll::Ready(Some(Ok(Frame::data(Bytes::from(sse_frame(&e))))))
            }
            // The reader shut down: end the stream.
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        match self.ping.poll_tick(cx) {
            Poll::Ready(_) => Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b": ping\n\n"))))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_query_parsing() {
        assert_eq!(event_set(None).unwrap(), EventSet::ALL);
        let s = event_set(Some("events=state%2Citems")).unwrap();
        assert_eq!(s.names(), ["state", "items"]);
        assert_eq!(event_set(Some("x=1&events=log")).unwrap().names(), ["log"]);
        assert!(event_set(Some("events=nope")).is_err());
    }

    fn server(life: Arc<crate::lifetime::Lifetime>) -> Arc<Server> {
        use sonara_audio::TestOutput;
        use sonara_engine::fake::FakeEngine;
        use sonara_reader::{Config, ReaderHandle, Registry};
        let registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        Arc::new(Server::new(reader, "secret".into(), life))
    }

    // #247: a request that reached the runtime as the idle exit was decided
    // lost its connection (RemoteDisconnected) because the process ended
    // without answering it. It must get its E_BUSY reply first.
    #[test]
    fn a_request_waiting_when_the_exit_is_decided_gets_e_busy() {
        use crate::lifetime::{ExitReason, Lifetime};
        use std::io::{Read, Write};
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let life = Lifetime::new(Duration::from_secs(30), false);
        let server = server(life.clone());
        let listener = rt
            .block_on(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)))
            .unwrap();
        let addr = listener.local_addr().unwrap();
        // The client connects and sends before the runtime accepts: the
        // connection waits in the listen backlog.
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        stream
            .write_all(
                b"POST /v1/get HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\n\
                  Content-Type: application/json\r\nContent-Length: 16\r\n\r\n{\"key\":\"volume\"}",
            )
            .unwrap();
        life.request_exit(ExitReason::Idle);
        rt.block_on(async {
            let served = tokio::spawn(serve(listener, server.clone()));
            tokio::time::timeout(Duration::from_secs(5), served)
                .await
                .expect("serve returns once the exit is decided")
                .unwrap();
            life.requests_done(Duration::from_secs(5)).await;
        });
        // What main does next: return, which ends every task.
        drop(rt);
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 409"), "{reply:?}");
        assert!(reply.contains("E_BUSY"), "{reply:?}");
    }

    #[test]
    fn sse_frames_carry_the_event_name() {
        let e = WireEvent {
            name: "item",
            json: "{\"event\":\"item\"}".into(),
        };
        assert_eq!(sse_frame(&e), "event: item\ndata: {\"event\":\"item\"}\n\n");
    }
}
