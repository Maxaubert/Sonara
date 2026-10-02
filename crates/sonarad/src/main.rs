//! `sonarad.exe`: see the crate docs (`lib.rs`) and `docs/protocol-v1.md`.
//!
//! Exit codes: 0 clean exit (idle, takeover, Ctrl+C), 1 startup failure,
//! 2 bad command line, 3 another instance already runs for this user and
//! home.
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::args::{self, Command, OutputKind};
use sonarad::home::{self, Home};
use sonarad::instance::{self, AcquireError};
use sonarad::lifetime::{self, ExitReason, Lifetime};
use sonarad::protocol::{self, Server};
use sonarad::runtime_file::{self, RuntimeInfo};
use sonarad::{http, null_output::NullOutput, tcp, VERSION};
use std::net::Ipv4Addr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::net::TcpListener;

const EXIT_STARTUP: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_ALREADY_RUNNING: u8 = 3;

fn main() -> ExitCode {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(Command::Run(a)) => a,
        Ok(Command::Help) => {
            println!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!(
                "sonarad {VERSION} (protocol {}.{})",
                protocol::PROTOCOL_MAJOR,
                protocol::PROTOCOL_MINOR
            );
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("sonarad: {e}\n{}", args::USAGE);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match start(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("sonarad: {message}");
            ExitCode::from(code)
        }
    }
}

fn build_reader(engine: &str, output: OutputKind) -> Result<ReaderHandle, String> {
    let registry = if engine == "fake" {
        let mut r = Registry::default();
        r.register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .map_err(|e| e.to_string())?;
        r
    } else {
        sonara_reader::default_registry()
    };
    let mut config = Config::new(registry);
    config.engine = Some(engine.to_string());
    if output == OutputKind::Null {
        let (out, events) = NullOutput::new();
        config = config.with_output(Box::new(out), events);
    }
    ReaderHandle::new(config).map_err(|e| format!("cannot start the reader: {e}"))
}

fn start(args: args::Args) -> Result<(), (u8, String)> {
    let fail = |m: String| (EXIT_STARTUP, m);
    let home = home::resolve(args.home.as_deref()).map_err(fail)?;
    let sid = instance::user_sid().map_err(fail)?;
    let key = if home.is_default {
        None
    } else {
        Some(home::key(&home))
    };
    let name = instance::mutex_name(&sid, key.as_deref());
    let _instance = match instance::acquire(&name) {
        Ok(i) => i,
        Err(AcquireError::AlreadyRunning) => {
            let pid = runtime_file::read_pid(&home.runtime_json())
                .map(|p| format!(" (pid {p})"))
                .unwrap_or_default();
            return Err((
                EXIT_ALREADY_RUNNING,
                format!(
                    "another instance is already running for this user and home{pid}; \
                     connect to it through {}",
                    home.runtime_json().display()
                ),
            ));
        }
        Err(AcquireError::Os(e)) => return Err(fail(e)),
    };
    let token = instance::new_token().map_err(fail)?;
    let reader = build_reader(&args.engine, args.output).map_err(fail)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(format!("cannot start the runtime: {e}")))?;
    let result = rt.block_on(run(&args, &home, &sid, token, reader.clone()));
    reader.shutdown();
    rt.shutdown_timeout(Duration::from_millis(500));
    runtime_file::remove_if_ours(&home.runtime_json(), std::process::id());
    result.map_err(fail)
}

async fn run(
    args: &args::Args,
    home: &Home,
    sid: &str,
    token: String,
    reader: ReaderHandle,
) -> Result<(), String> {
    // Loopback only, never another address (spec section 4).
    let tcp_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| format!("cannot listen on 127.0.0.1: {e}"))?;
    let http_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| format!("cannot listen on 127.0.0.1: {e}"))?;
    let port = tcp_listener.local_addr().map_err(|e| e.to_string())?.port();
    let http_port = http_listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();

    let life = Lifetime::new(args.idle_exit, args.standalone);
    let server = Arc::new(Server::new(reader.clone(), token.clone(), life.clone()));
    tokio::spawn(tcp::serve(tcp_listener, server.clone()));
    tokio::spawn(http::serve(http_listener, server.clone()));
    let busy_server = server.clone();
    tokio::spawn(lifetime::monitor(
        life.clone(),
        Duration::from_millis(100),
        move || busy_server.is_reading(),
    ));

    let info = RuntimeInfo {
        pid: std::process::id(),
        port,
        http_port,
        token,
        version: VERSION.to_string(),
        protocol: (protocol::PROTOCOL_MAJOR, protocol::PROTOCOL_MINOR),
        capabilities: protocol::CAPABILITIES
            .iter()
            .map(|s| s.to_string())
            .collect(),
        started_at: runtime_file::rfc3339(SystemTime::now()),
    };
    runtime_file::write(&home.runtime_json(), &info, |p| {
        instance::restrict_to_user(p, sid)
    })?;
    eprintln!(
        "sonarad {VERSION}: listening on 127.0.0.1:{port} (tcp) and 127.0.0.1:{http_port} (http), \
         home {}",
        home.dir.display()
    );

    let why = tokio::select! {
        why = life.wait_exit() => why,
        _ = tokio::signal::ctrl_c() => ExitReason::Signal,
    };
    // Remove the file first: a client waiting on a takeover sees it go, then
    // the process exit.
    runtime_file::remove_if_ours(&home.runtime_json(), std::process::id());
    eprintln!("sonarad: exiting ({why:?})");
    Ok(())
}
