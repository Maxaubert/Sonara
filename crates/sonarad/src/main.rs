//! `sonarad.exe`: see the crate docs (`lib.rs`) and `docs/protocol-v1.md`.
//!
//! Exit codes: 0 clean exit (idle, takeover, Ctrl+C), 1 startup failure,
//! 2 bad command line, 3 another instance already runs for this user and
//! home.
use sonara_engine::kokoro::{self, Kokoro};
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::args::{self, Command, OutputKind, SystemKind};
use sonarad::home::{self, Home};
use sonarad::instance::{self, AcquireError};
use sonarad::lifetime::{self, ExitReason, Lifetime};
use sonarad::protocol::{self, Server};
use sonarad::runtime_file::{self, RuntimeInfo};
use sonarad::system_ext::SystemHost;
use sonarad::{http, null_output::NullOutput, tcp, VERSION};
use std::net::Ipv4Addr;
use std::path::PathBuf;
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

/// `onnxruntime.dll` next to `sonarad.exe`; `SONARA_ORT_DYLIB` overrides
/// it (a development aid: `cargo run` builds have no DLL next to them).
fn onnxruntime_dll() -> PathBuf {
    if let Some(p) = std::env::var_os("SONARA_ORT_DYLIB").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("onnxruntime.dll")))
        .unwrap_or_else(|| PathBuf::from("onnxruntime.dll"))
}

/// The engines and the one to start with. `fake` runs alone (tests,
/// conformance: no Kokoro, so nothing is ever downloaded). Otherwise OneCore
/// and Kokoro, with OneCore speaking while Kokoro is not ready; without a
/// named engine, Kokoro when ONNX Runtime is installed, else OneCore.
fn build_registry(
    engine: Option<&str>,
    home: &Home,
) -> Result<(Registry, String, Option<Kokoro>), String> {
    if engine == Some("fake") {
        let mut r = Registry::default();
        r.register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .map_err(|e| e.to_string())?;
        return Ok((r, "fake".into(), None));
    }
    let mut registry = sonara_reader::default_registry();
    let runtime = onnxruntime_dll();
    let mut config = kokoro::Config::new(
        home.models().join(kokoro::download::MODEL_SUBDIR),
        runtime.clone(),
    );
    config.fallback = registry.get(sonara_engine::onecore::ID.as_str()).ok();
    let has_onecore = config.fallback.is_some();
    let k = Kokoro::new(config);
    registry
        .register(Arc::new(k.clone()))
        .map_err(|e| e.to_string())?;
    let chosen = match engine {
        Some(e) => e.to_string(),
        None if runtime.is_file() || !has_onecore => kokoro::ID.to_string(),
        None => sonara_engine::onecore::ID.to_string(),
    };
    Ok((registry, chosen, Some(k)))
}

fn build_reader(
    engine: Option<&str>,
    output: OutputKind,
    home: &Home,
) -> Result<ReaderHandle, String> {
    let (registry, engine, kokoro) = build_registry(engine, home)?;
    let mut config = Config::new(registry);
    config.engine = Some(engine.clone());
    if output == OutputKind::Null {
        let (out, events) = NullOutput::new();
        config = config.with_output(Box::new(out), events);
    }
    let reader = ReaderHandle::new(config).map_err(|e| format!("cannot start the reader: {e}"))?;
    // Prefetch: verify, download (in the background) and load the model now,
    // so the first sentence is soon Kokoro's.
    if let (Some(k), true) = (kokoro, engine == kokoro::ID.as_str()) {
        k.prepare();
    }
    Ok(reader)
}

/// The platform of the `system` extension.
fn system_platform(kind: SystemKind, home: &Home) -> Option<sonara_system::Platform> {
    match kind {
        SystemKind::Fake => {
            Some(sonara_system::fake::Fake::file(home.dir.join("fake-system.json")).platform())
        }
        #[cfg(windows)]
        SystemKind::Windows => Some(sonara_system::win::platform()),
        #[cfg(not(windows))]
        SystemKind::Windows => None,
    }
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
    let instance = match instance::acquire(&name) {
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
    let reader = build_reader(args.engine.as_deref(), args.output, &home).map_err(fail)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(format!("cannot start the runtime: {e}")))?;
    let result = rt.block_on(run(&args, &home, &sid, token, reader.clone()));
    reader.shutdown();
    rt.shutdown_timeout(Duration::from_millis(500));
    // Release the instance lock before runtime.json goes: a client that
    // relaunches as soon as the file is gone (instead of waiting for the
    // pid) then starts rather than exiting with code 3. A new instance that
    // wins the lock meanwhile writes its own file, which this one leaves.
    drop(instance);
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
    let mut server = Server::new(reader.clone(), token.clone(), life.clone());
    if let Some(platform) = system_platform(args.system, home) {
        server = server.with_system(SystemHost {
            platform,
            home: home.dir.clone(),
            http_port,
            token: token.clone(),
        });
    }
    let server = Arc::new(server);
    // The startup sweep: restore what a previous runtime that died left
    // ducked or paused (never strand other apps, #131).
    if let Some(s) = server.system().cloned() {
        let _ = tokio::task::spawn_blocking(move || s.recover()).await;
    }
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
        extensions: server.offered().iter().map(|s| s.to_string()).collect(),
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
    // `start` removes runtime.json once speech stopped and the instance
    // lock is released.
    eprintln!("sonarad: exiting ({why:?})");
    // Release the hotkeys and put other apps' audio back before exiting.
    if let Some(s) = server.system().cloned() {
        let _ = tokio::task::spawn_blocking(move || s.shutdown()).await;
    }
    Ok(())
}
