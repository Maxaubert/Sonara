//! `sonarad.exe`: see the crate docs (`lib.rs`) and `docs/protocol-v1.md`.
//!
//! Exit codes: 0 clean exit (idle, takeover, Ctrl+C), 1 startup failure,
//! 2 bad command line, 3 another instance already runs for this user and
//! home.
use sonara_engine::external::keys::{CredentialStore, FileStore, KeyStore};
use sonara_engine::kokoro::{self, Kokoro};
use sonara_engine::{Engine, LicenseClass};
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::args::{self, Command, KeysKind, OutputKind, SystemKind};
use sonarad::config::{self, Store};
use sonarad::engines::{self, Engines};
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

/// The engines, the one to start with, and what external engine profiles
/// speak with when they cannot.
struct Built {
    registry: Arc<Registry>,
    /// The saved or named engine, else the default choice.
    chosen: String,
    /// Kokoro when ONNX Runtime is installed (or there is no OneCore), else
    /// OneCore; `fake` in test runs.
    default_engine: String,
    kokoro: Option<Kokoro>,
    /// What a profile speaks with when it cannot (Kokoro, or the fake
    /// engine), and its voice.
    fallback: Option<Arc<dyn Engine>>,
    fallback_voice: String,
}

/// The licence classes this runtime allows: external engines unless
/// `--no-external-engines`.
fn classes(external: bool) -> Vec<LicenseClass> {
    let mut c = vec![LicenseClass::Permissive, LicenseClass::Os];
    if external {
        c.push(LicenseClass::External);
    }
    c
}

/// The engines and the one to start with. `fake` runs alone (tests,
/// conformance: no Kokoro, so nothing is ever downloaded). Otherwise OneCore
/// and Kokoro, with OneCore speaking while Kokoro is not ready; without a
/// named engine, Kokoro when ONNX Runtime is installed, else OneCore.
fn build_registry(engine: Option<&str>, home: &Home, external: bool) -> Result<Built, String> {
    let allowed = classes(external);
    if engine == Some("fake") {
        let r = Registry::new(&allowed);
        let fake: Arc<dyn Engine> = Arc::new(sonara_engine::fake::FakeEngine::new());
        r.register(fake.clone()).map_err(|e| e.to_string())?;
        return Ok(Built {
            registry: Arc::new(r),
            chosen: "fake".into(),
            default_engine: "fake".into(),
            kokoro: None,
            fallback: Some(fake),
            fallback_voice: String::new(),
        });
    }
    let registry = sonara_reader::registry_with(&allowed);
    let runtime = onnxruntime_dll();
    let mut config = kokoro::Config::new(
        home.models().join(kokoro::download::MODEL_SUBDIR),
        runtime.clone(),
    );
    config.fallback = registry.get(sonara_engine::onecore::ID.as_str()).ok();
    let has_onecore = config.fallback.is_some();
    let k = Kokoro::new(config);
    let shared: Arc<dyn Engine> = Arc::new(k.clone());
    registry
        .register(shared.clone())
        .map_err(|e| e.to_string())?;
    let default_engine = if runtime.is_file() || !has_onecore {
        kokoro::ID.to_string()
    } else {
        sonara_engine::onecore::ID.to_string()
    };
    let fallback_voice = if shared.voices().iter().any(|v| v.id == "af_sarah") {
        "af_sarah".to_string()
    } else {
        String::new()
    };
    Ok(Built {
        registry: Arc::new(registry),
        chosen: engine.map(str::to_string).unwrap_or(default_engine.clone()),
        default_engine,
        kokoro: Some(k),
        fallback: Some(shared),
        fallback_voice,
    })
}

/// Engines for voice previews: their own OneCore (a preview never waits
/// for or cancels the reader's OneCore synthesis), and the reader's Kokoro
/// itself rather than a second copy: one model in memory and one download
/// manager per model folder. A preview on Kokoro waits at most for the
/// sentence the reader is synthesizing, and a skip on the reader cancels it.
/// External engine profiles are added by `Engines::attach`.
fn preview_registry(
    engine: &str,
    kokoro: Option<&Kokoro>,
    external: bool,
) -> Option<Arc<Registry>> {
    let allowed = classes(external);
    if engine == "fake" {
        let r = Registry::new(&allowed);
        r.register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .ok()?;
        return Some(Arc::new(r));
    }
    let r = sonara_reader::registry_with(&allowed);
    if let Some(k) = kokoro {
        r.register(Arc::new(k.clone())).ok()?;
    }
    Some(Arc::new(r))
}

/// The reader on `built`, starting with `engine`.
fn build_reader(built: &Built, engine: &str, output: OutputKind) -> Result<ReaderHandle, String> {
    let mut config = Config::new(built.registry.clone());
    config.engine = Some(engine.to_string());
    if output == OutputKind::Null {
        let (out, events) = NullOutput::new();
        config = config.with_output(Box::new(out), events);
    }
    ReaderHandle::new(config).map_err(|e| format!("cannot start the reader: {e}"))
}

/// Where profile keys are kept.
fn key_store(kind: KeysKind, home: &Home) -> Arc<dyn KeyStore> {
    match kind {
        KeysKind::Windows => Arc::new(CredentialStore),
        KeysKind::Fake => Arc::new(FileStore::new(home.dir.join("fake-keys.json"))),
    }
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
    let wanted = args.engine.clone().or(saved.clone());
    let built = build_registry(wanted.as_deref(), &home, args.external_engines).map_err(fail)?;
    // External engine profiles, registered before the reader starts so a
    // saved `engine` naming one works on the first sentence.
    let engines = args.external_engines.then(|| {
        let log_home = home.clone();
        let (e, problems) = Engines::load(engines::Setup {
            home: home.dir.clone(),
            store: key_store(args.keys, &home),
            fallback: built.fallback.clone(),
            fallback_voice: built.fallback_voice.clone(),
            default_engine: built.default_engine.clone(),
            log: Some(Arc::new(move |line: &str| log_home.log(line))),
        });
        for p in problems {
            home.log(&p);
        }
        e.attach(built.registry.clone());
        e
    });
    // A test run (`--engine fake`) keeps a saved external engine; the fake
    // engine is its fallback.
    let chosen = match (args.engine.as_deref(), &saved, &engines) {
        (Some("fake"), Some(s), Some(x)) if x.get(s).is_some() => s.clone(),
        _ => built.chosen.clone(),
    };
    let (reader, engine) = match build_reader(&built, &chosen, args.output) {
        Ok(r) => (r, chosen.clone()),
        Err(e) if args.engine.is_none() && wanted.is_some() => {
            home.log(&format!(
                "config.json: engine '{}' not available ({e}); using the default",
                wanted.as_deref().unwrap_or_default()
            ));
            let r = build_reader(&built, &built.default_engine, args.output).map_err(fail)?;
            (r, built.default_engine.clone())
        }
        Err(e) => return Err(fail(e)),
    };
    // Prefetch: verify, download (in the background) and load the model now,
    // so the first sentence is soon Kokoro's.
    if let (Some(k), true) = (&built.kokoro, engine == kokoro::ID.as_str()) {
        k.prepare();
    }
    let kokoro = built.kokoro.clone();
    // Before the first client: nothing speaks with the defaults first.
    for p in config::apply_reader(&store, &reader, false) {
        home.log(&p);
    }
    // One line per start for support, then the model's readiness changes.
    support_log::log_startup(&home, VERSION, &reader, &engine);
    support_log::watch_engine(&home, &reader);
    // Previews use the engines of the reader actually started.
    let previews = preview_registry(&engine, kokoro.as_ref(), args.external_engines);
    if let (Some(e), Some(p)) = (&engines, &previews) {
        e.attach(p.clone());
    }
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
        engines,
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
    previews: Option<Arc<Registry>>,
    kokoro: Option<Kokoro>,
    engines: Option<Arc<Engines>>,
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
    if let Some(e) = engines {
        server = server.with_engines(e);
    }
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
    let http_task = tokio::spawn(http::serve(http_listener, server.clone()));

    let info = RuntimeInfo {
        pid: std::process::id(),
        port,
        http_port,
        token,
        version: VERSION.to_string(),
        protocol: (protocol::PROTOCOL_MAJOR, protocol::PROTOCOL_MINOR),
        capabilities: server
            .capabilities()
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
        _ = tokio::signal::ctrl_c() => {
            life.request_exit(ExitReason::Signal);
            ExitReason::Signal
        }
    };
    // Answer the HTTP requests that arrived as the exit was decided
    // (`E_BUSY`) before the process ends, so none loses its connection
    // without a reply (#247). Both waits are bounded.
    let _ = tokio::time::timeout(Duration::from_secs(2), http_task).await;
    life.requests_done(Duration::from_secs(2)).await;
    // `start` removes runtime.json once speech stopped and the instance
    // lock is released.
    eprintln!("sonarad: exiting ({why:?})");
    // Release the hotkeys and put other apps' audio back before exiting.
    if let Some(s) = server.system().cloned() {
        let _ = tokio::task::spawn_blocking(move || s.shutdown()).await;
    }
    Ok(())
}
