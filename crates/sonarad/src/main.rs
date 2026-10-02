//! `sonarad.exe`: see the crate docs (`lib.rs`) and `docs/protocol-v1.md`.
//!
//! Exit codes: 0 clean exit (idle, takeover, Ctrl+C), 1 startup failure,
//! 2 bad command line, 3 another instance already runs for this user and
//! home.
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::args::{self, Command, OutputKind, SystemKind};
use sonarad::config::{self, Store};
use sonarad::home::{self, Home};
use sonarad::instance::{self, AcquireError};
use sonarad::lifetime::{self, ExitReason, Lifetime};
use sonarad::migrate;
use sonarad::protocol::{self, Server};
use sonarad::runtime_file::{self, RuntimeInfo};
use sonarad::system_ext::SystemHost;
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

/// The engines of a run: the fake engine for `--engine fake`, else the
/// default ones (OneCore on Windows).
fn build_registry(engine: &str) -> Result<Registry, String> {
    Ok(if engine == "fake" {
        let mut r = Registry::default();
        r.register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .map_err(|e| e.to_string())?;
        r
    } else {
        sonara_reader::default_registry()
    })
}

fn build_reader(engine: &str, output: OutputKind) -> Result<ReaderHandle, String> {
    let registry = build_registry(engine)?;
    let mut config = Config::new(registry);
    config.engine = Some(engine.to_string());
    if output == OutputKind::Null {
        let (out, events) = NullOutput::new();
        config = config.with_output(Box::new(out), events);
    }
    ReaderHandle::new(config).map_err(|e| format!("cannot start the reader: {e}"))
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
    // The Python plugin's settings, once, for the default home (or the
    // folder given with --migrate-from).
    let legacy = args
        .migrate_from
        .clone()
        .or_else(|| home.is_default.then(migrate::default_legacy_dir).flatten());
    if let Some(dir) = legacy {
        for note in migrate::run(&home.dir, &dir).unwrap_or_default() {
            home.log(&format!("migration: {note}"));
        }
    }
    let (store, problems) = Store::load(&home.dir);
    for p in problems {
        home.log(&p);
    }
    // The saved engine starts the reader unless --engine chose one.
    let engine = if args.engine_given {
        args.engine.clone()
    } else {
        store
            .user("engine")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| args.engine.clone())
    };
    let (reader, engine) = match build_reader(&engine, args.output) {
        Ok(r) => (r, engine),
        Err(e) if engine != args.engine => {
            home.log(&format!(
                "config.json: engine '{engine}' not available ({e}); using '{}'",
                args.engine
            ));
            let r = build_reader(&args.engine, args.output).map_err(fail)?;
            (r, args.engine.clone())
        }
        Err(e) => return Err(fail(e)),
    };
    // Before the first client: nothing speaks with the defaults first.
    for p in config::apply_reader(&store, &reader, false) {
        home.log(&p);
    }
    // Previews use the engines of the reader actually started.
    let previews = build_registry(&engine).ok();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| fail(format!("cannot start the runtime: {e}")))?;
    let result = rt.block_on(run(
        &args,
        &home,
        &sid,
        token,
        reader.clone(),
        store,
        previews,
    ));
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
    store: Arc<Store>,
    previews: Option<Registry>,
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
    let mut server = Server::new(reader.clone(), token.clone(), life.clone()).with_config(store);
    if let Some(platform) = system_platform(args.system, home) {
        server = server.with_system(SystemHost {
            platform,
            home: home.dir.clone(),
            http_port,
            token: token.clone(),
            previews,
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
