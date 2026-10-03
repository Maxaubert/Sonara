//! `sonarad.exe`: see the crate docs (`lib.rs`) and `docs/protocol-v1.md`.
//!
//! Exit codes: 0 clean exit (idle, takeover, Ctrl+C), 1 startup failure,
//! 2 bad command line, 3 another instance already runs for this user and
//! home.
use sonara_engine::kokoro::{self, Kokoro};
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
use sonarad::{http, null_output::NullOutput, support_log, tcp, trace_log, VERSION};
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

/// Engines for voice previews: their own OneCore (a preview never waits
/// for or cancels the reader's OneCore synthesis), and the reader's Kokoro
/// itself rather than a second copy: one model in memory and one download
/// manager per model folder. A preview on Kokoro waits at most for the
/// sentence the reader is synthesizing, and a skip on the reader cancels it.
fn preview_registry(engine: &str, kokoro: Option<&Kokoro>) -> Option<Registry> {
    if engine == "fake" {
        let mut r = Registry::default();
        r.register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .ok()?;
        return Some(r);
    }
    let mut r = sonara_reader::default_registry();
    if let Some(k) = kokoro {
        r.register(Arc::new(k.clone())).ok()?;
    }
    Some(r)
}

/// The reader, the Kokoro engine (unless `fake`) and the id of the engine
/// it started with.
fn build_reader(
    engine: Option<&str>,
    output: OutputKind,
    home: &Home,
) -> Result<(ReaderHandle, Option<Kokoro>, String), String> {
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
    if let (Some(k), true) = (&kokoro, engine == kokoro::ID.as_str()) {
        k.prepare();
    }
    Ok((reader, kokoro, engine))
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
    // Text and payloads in the troubleshooting log (#219).
    trace_log::set_debug(store.value(trace_log::DEBUG_KEY).as_bool().unwrap_or(true));
    // --engine, else the saved engine, else the default choice (Kokoro
    // when ONNX Runtime is installed, else OneCore).
    let saved = store
        .user("engine")
        .and_then(|v| v.as_str().map(str::to_string));
    let wanted = args.engine.clone().or(saved);
    let (reader, kokoro, engine) = match build_reader(wanted.as_deref(), args.output, &home) {
        Ok(r) => r,
        Err(e) if args.engine.is_none() && wanted.is_some() => {
            home.log(&format!(
                "config.json: engine '{}' not available ({e}); using the default",
                wanted.as_deref().unwrap_or_default()
            ));
            build_reader(None, args.output, &home).map_err(fail)?
        }
        Err(e) => return Err(fail(e)),
    };
    // Before the first client: nothing speaks with the defaults first.
    for p in config::apply_reader(&store, &reader, false) {
        home.log(&p);
    }
    // One line per start for support, then the model's readiness changes.
    support_log::log_startup(&home, VERSION, &reader, &engine);
    support_log::watch_engine(&home, &reader);
    // Previews use the engines of the reader actually started.
    let previews = preview_registry(&engine, kokoro.as_ref());
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
        kokoro,
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

#[allow(clippy::too_many_arguments)]
async fn run(
    args: &args::Args,
    home: &Home,
    sid: &str,
    token: String,
    reader: ReaderHandle,
    store: Arc<Store>,
    previews: Option<Registry>,
    kokoro: Option<Kokoro>,
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
    // Custom earcons: the folder is created so the user can drop WAVs in.
    let _ = std::fs::create_dir_all(home.earcons());
    let log_home = home.clone();
    let earcons = sonara_agent::Library::new(
        home.earcons(),
        Some(Arc::new(move |line: &str| log_home.log(line))),
    );
    let activity_home = home.clone();
    let mut server = Server::new(reader.clone(), token.clone(), life.clone())
        .with_config(store)
        .with_log(Arc::new(move |line: &str| activity_home.log(line)))
        .with_earcons(Arc::new(earcons));
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
    // Reading start, text and end in the support log, with the session
    // and origin of each item (#217, #219), and the spoken cues.
    let tags = Arc::downgrade(&server);
    support_log::watch_reading(
        home,
        &reader,
        Box::new(move |id| {
            tags.upgrade()
                .and_then(|s| s.channels().and_then(|c| c.tag(id)))
        }),
        server.origins(),
    );
    if let Some(s) = server.system() {
        support_log::watch_cues(home, s.cues());
    }
    // The startup sweep: restore what a previous runtime that died left
    // ducked or paused (never strand other apps, #131).
    if let Some(s) = server.system().cloned() {
        let _ = tokio::task::spawn_blocking(move || s.recover()).await;
    }
    tokio::spawn(tcp::serve(tcp_listener, server.clone()));
    tokio::spawn(http::serve(http_listener, server.clone()));

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
    // The idle countdown starts once clients can find the runtime: a slow
    // start (the startup sweep, the migration) must not use up the idle
    // time before the first client could connect (#194).
    life.touch();
    // Reading, or fetching the Kokoro model: an idle exit mid-download
    // would leave the first install without Kokoro for longer.
    let busy_server = server.clone();
    let retire_server = server.clone();
    tokio::spawn(lifetime::monitor(
        life.clone(),
        Duration::from_millis(100),
        move || busy_server.is_reading() || kokoro.as_ref().is_some_and(Kokoro::is_preparing),
        move || retire_server.retire_if_idle(),
    ));
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
