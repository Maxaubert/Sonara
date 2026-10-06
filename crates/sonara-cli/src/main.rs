//! `sonara.exe`: see the library docs. Exit codes: 0 done, 1 failed (for
//! `doctor`: a `FAIL` row), 2 bad command line.
use serde_json::{json, Value};
use sonara_cli::client::attach;
use sonara_cli::doctor::{self, Row, Status};
use sonara_cli::engines::{self, Action};
use sonara_cli::engines_file;
use sonara_cli::lifecycle::{self, Running, Stopped};
use sonara_cli::paths::{self, Paths};
use sonara_cli::uninstall::{self, Keep};
use sonara_cli::VERSION;
use sonara_client::{runtime_args, RUNTIME_EXE, STOPPED};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "usage: sonara <command>

  start              start Sonara (and keep it on; clears a stop)
  stop               stop Sonara; it stays off until `sonara start`
  settings           open the settings page in the browser (starts Sonara)
  doctor             check the install and the runtime
  uninstall [--keep LIST]
                     stop Sonara and remove its runtime and files; LIST is
                     any of settings, models, logs (comma-separated) or
                     none (default: settings; settings include the engines
                     you added and their keys)
  engines ...        the speech engines you added (sonara engines help)
  version            print the version

Environment: SONARA_HOME (the home, default %LOCALAPPDATA%\\Sonara),
SONARA_NO_BROWSER (print the settings URL only), SONARA_RUNTIME_ARGS
(extra sonarad arguments).";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let exe = std::env::current_exe().unwrap_or_else(|_| "sonara.exe".into());
    let paths = match paths::resolve(&env, &exe) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sonara: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rest = &args[1..];
    let result = match cmd {
        "start" => start(&paths),
        "stop" => stop(&paths),
        "settings" => settings(&paths),
        "doctor" => return doctor(&paths),
        "engines" => match rest.first().map(String::as_str) {
            None | Some("help") | Some("--help") | Some("-h") => {
                println!("{}", engines::USAGE);
                Ok(())
            }
            _ => match engines::parse(rest) {
                Ok(action) => engines_cmd(&paths, action),
                Err(e) => {
                    eprintln!("sonara: {e}\n{}", engines::USAGE);
                    return ExitCode::from(2);
                }
            },
        },
        "uninstall" => match parse_uninstall(rest) {
            Ok(keep) => uninstall(&paths, &exe, &keep),
            Err(e) => {
                eprintln!("sonara: {e}\n{USAGE}");
                return ExitCode::from(2);
            }
        },
        "version" | "--version" | "-V" => {
            println!("sonara {VERSION}");
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("sonara: unknown command '{other}'\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sonara: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_uninstall(rest: &[String]) -> Result<Vec<Keep>, String> {
    let mut keep = uninstall::DEFAULT_KEEP.to_vec();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--keep=") {
            keep = uninstall::parse_keep(v)?;
        } else if a == "--keep" {
            keep = uninstall::parse_keep(it.next().ok_or("--keep needs a value")?)?;
        } else {
            return Err(format!("unknown argument '{a}'"));
        }
    }
    Ok(keep)
}

/// Start this release's runtime if needed, then remove older version
/// folders; `report` prints what was replaced and removed.
fn ensure(paths: &Paths, report: bool) -> Result<Running, String> {
    let r = lifecycle::ensure_running(&paths.home, &paths.exe_dir, &runtime_args(&env))?;
    let (removed, failed) = lifecycle::remove_old_versions(paths);
    if !report {
        return Ok(r);
    }
    if let Some(old) = &r.replaced {
        println!("Replaced Sonara {old} with {VERSION}.");
    }
    for d in removed {
        println!("Removed the old runtime {}", d.display());
    }
    for (d, e) in failed {
        println!(
            "Could not remove {} yet ({e}); the next start tries again.",
            d.display()
        );
    }
    Ok(r)
}

fn start(paths: &Paths) -> Result<(), String> {
    let r = ensure(paths, true)?;
    let pid = r.rt.pid.unwrap_or(0);
    if r.started {
        println!("Sonara {} started (pid {pid}).", r.version);
    } else {
        println!("Sonara {} is already running (pid {pid}).", r.version);
    }
    Ok(())
}

fn stop(paths: &Paths) -> Result<(), String> {
    match lifecycle::stop(&paths.home)? {
        Stopped::Exited(pid) => println!("Sonara stopped (pid {}).", pid.unwrap_or(0)),
        Stopped::NotRunning => println!("Sonara was not running."),
    }
    println!("It stays off until /sonara:start (the hooks will not start it).");
    Ok(())
}

/// Open `url` in the default browser (not with `SONARA_NO_BROWSER`).
fn open_url(url: &str) -> bool {
    if env("SONARA_NO_BROWSER").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .is_ok()
}

fn settings(paths: &Paths) -> Result<(), String> {
    let mut r = ensure(paths, true)?;
    let url = r
        .conn
        .get("settings_url")
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or("the runtime gave no settings page address")?;
    if open_url(&url) {
        println!("Opened the Sonara settings page in your browser: {url}");
    } else {
        println!("Sonara settings page: {url}");
    }
    Ok(())
}

/// `sonara engines ...`: one protocol request to the running runtime
/// (started if needed).
fn engines_cmd(paths: &Paths, action: Action) -> Result<(), String> {
    let request = match action {
        Action::List => json!({"type": "engine_list"}),
        Action::Request(r) => r,
        Action::AddLocal { profile, replace } => return add_local(paths, &profile, replace),
        Action::SetKey { id } => {
            let key = engines::read_key()?;
            json!({"type": "engine_key", "engine": id, "secret": key})
        }
    };
    let mut r = ensure(paths, false)?;
    // A test or a voice list may wait for the provider.
    let reply = r
        .conn
        .request_timeout(request.clone(), Duration::from_secs(150))
        .map_err(|e| format!("no answer from the runtime: {e}"))?;
    if reply["ok"] != true {
        if reply["error"]["code"] == "E_UNKNOWN_TYPE" || reply["error"]["code"] == "E_UNSUPPORTED" {
            return Err(format!(
                "{} (this runtime has no external engines)",
                engines::error_line(&reply)
            ));
        }
        return Err(engines::error_line(&reply));
    }
    if request["type"] == "engine_list" {
        for line in engines::list_lines(&reply) {
            println!("{line}");
        }
    } else {
        println!("{}", engines::done_line(&request, &reply));
    }
    match engines::fetch_error(&request, &reply) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `sonara engines add <id> --kind command`: written into `engines.json`
/// as the user, then the runtime reads the file again (`engine_reload`,
/// which takes no profile). An entry the runtime cannot use is taken out
/// again.
fn add_local(paths: &Paths, profile: &Value, replace: bool) -> Result<(), String> {
    let id = profile["id"].as_str().unwrap_or("?").to_string();
    let written = engines_file::write(&paths.home, profile, replace)?;
    let reload = |r: &mut Running| {
        r.conn
            .request_timeout(json!({"type": "engine_reload"}), Duration::from_secs(30))
            .map_err(|e| format!("no answer from the runtime: {e}"))
    };
    let mut r = match ensure(paths, false) {
        Ok(r) => r,
        Err(e) => {
            println!(
                "Added {id} to {}; the runtime did not start ({e}), it reads it when it starts.",
                written.file.display()
            );
            return Ok(());
        }
    };
    let reply = reload(&mut r)?;
    let view = if reply["ok"] == true {
        engines_file::reloaded_view(&reply, &id)
    } else {
        Err(engines::error_line(&reply))
    };
    match view {
        Ok(view) => {
            let request = json!({"type": "engine_reload"});
            println!("{}", engines::done_line(&request, &json!({"engine": view})));
            Ok(())
        }
        Err(e) => {
            written.undo()?;
            let _ = reload(&mut r);
            Err(e)
        }
    }
}

fn file_row(dir: &Path, name: &str, missing: Status, why: &str) -> Row {
    let p = dir.join(name);
    if p.is_file() {
        Row::new(Status::Ok, name, p.display().to_string())
    } else {
        Row::new(
            missing,
            name,
            format!("missing from {} ({why})", dir.display()),
        )
    }
}

/// The plugin's hooks, when run from the plugin (`CLAUDE_PLUGIN_ROOT`).
fn hooks_row() -> Row {
    let Some(root) = env("CLAUDE_PLUGIN_ROOT").filter(|r| !r.is_empty()) else {
        return Row::new(
            Status::Info,
            "hooks",
            "run /sonara:doctor in Claude Code to check the plugin's hooks",
        );
    };
    let root = Path::new(&root);
    let hooks = root.join("hooks").join("hooks.json");
    let text = std::fs::read_to_string(&hooks).unwrap_or_default();
    if !text.contains("sonara-hook-launch") {
        return Row::new(
            Status::Warn,
            "hooks",
            format!("{} does not call bin/sonara-hook-launch", hooks.display()),
        );
    }
    let wanted = std::fs::read_to_string(root.join("bin").join("runtime-version"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !wanted.is_empty() && !lifecycle::serves(&wanted, VERSION) {
        return Row::new(
            Status::Warn,
            "hooks",
            format!("the plugin needs runtime {wanted}, this is {VERSION}; start a new Claude Code session to install it"),
        );
    }
    Row::new(
        Status::Ok,
        "hooks",
        format!("{} (runtime {VERSION})", hooks.display()),
    )
}

/// The Python plugin's daemon, which would hold the same hotkeys.
fn legacy_row() -> Option<Row> {
    let profile = env("USERPROFILE").filter(|p| !p.is_empty())?;
    let dir = Path::new(&profile).join(".sonara");
    let app = dir.join("app");
    (app.is_dir() || dir.join("daemon.lock").is_file()).then(|| {
        Row::new(
            Status::Warn,
            "python sonara",
            format!(
                "the Python Sonara is still set up in {} (its daemon and autostart can hold the \
                 hotkeys and speak twice). Remove it once from Git Bash: \
                 PYTHONPATH=~/.sonara/app python -m sonara.cli uninstall",
                dir.display()
            ),
        )
    })
}

fn doctor(paths: &Paths) -> ExitCode {
    let mut rows = vec![
        Row::new(Status::Info, "version", format!("sonara {VERSION}")),
        Row::new(Status::Info, "home", paths.home.display().to_string()),
        file_row(
            &paths.exe_dir,
            RUNTIME_EXE,
            Status::Fail,
            "reinstall: /sonara:start",
        ),
        file_row(
            &paths.exe_dir,
            "sonara-hook.exe",
            Status::Fail,
            "reinstall: /sonara:start",
        ),
        file_row(
            &paths.exe_dir,
            "onnxruntime.dll",
            Status::Warn,
            "Kokoro cannot run; Windows voices only",
        ),
    ];
    let stopped = paths.home.join(STOPPED).exists();
    let running = if stopped {
        match attach(&paths.home, &[], false) {
            Some((rt, conn, h)) => {
                rows.push(Row::new(
                    Status::Warn,
                    "runtime",
                    format!(
                        "running (version {}, pid {}) but shut down for the hooks; run /sonara:start",
                        h["version"].as_str().unwrap_or("?"),
                        rt.pid.unwrap_or(0)
                    ),
                ));
                Some(conn)
            }
            None => {
                rows.push(Row::new(
                    Status::Warn,
                    "runtime",
                    "stopped; run /sonara:start",
                ));
                None
            }
        }
    } else {
        match ensure(paths, false) {
            Ok(r) => {
                let how = if r.started { "started now" } else { "running" };
                rows.push(Row::new(
                    Status::Ok,
                    "runtime",
                    format!(
                        "{how}, version {}, pid {}",
                        r.version,
                        r.rt.pid.unwrap_or(0)
                    ),
                ));
                Some(r.conn)
            }
            Err(e) => {
                rows.push(Row::new(Status::Fail, "runtime", e));
                None
            }
        }
    };
    if let Some(mut c) = running {
        // The state snapshot carries the engine's readiness.
        let state = c
            .request(json!({"type": "subscribe", "events": ["state"]}))
            .ok()
            .and_then(|_| c.event(Duration::from_secs(3)).ok());
        if let Some(s) = &state {
            rows.push(doctor::engine_row(&s["engine_status"]));
            let voice = s["voice"].as_str().unwrap_or("the engine's default");
            rows.push(Row::new(
                Status::Info,
                "voice",
                format!(
                    "{voice} at {} words per minute, volume {} %",
                    s["rate"], s["volume"]
                ),
            ));
        }
        let _ = c.request(json!({"type": "subscribe", "events": []}));
        if let Ok(v) = c.request(json!({"type": "voices"})) {
            rows.push(doctor::voices_row(&v["voices"]));
        }
        if let Some(h) = c.get("hotkeys") {
            rows.push(doctor::hotkeys_row(&h));
        }
        if let (Some(m), Some(l)) = (c.get("audio_mode"), c.get("duck_level")) {
            rows.push(doctor::audio_row(&m, &l));
        }
        if let Some(s) = c.get("summaries") {
            let on = s["enabled"] == true;
            rows.push(Row::new(
                Status::Info,
                "summaries",
                if on {
                    format!(
                        "on ({}, {})",
                        s["style"].as_str().unwrap_or("?"),
                        s["model"].as_str().unwrap_or("?")
                    )
                } else {
                    "off: replies are read as they are".to_string()
                },
            ));
        }
        if let Some(Value::Number(n)) = c.get("mute_level") {
            rows.push(Row::new(Status::Info, "mute level", n.to_string()));
        }
    }
    rows.push(hooks_row());
    rows.extend(legacy_row());
    for r in &rows {
        println!("{r}");
    }
    if doctor::failed(&rows) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn uninstall(paths: &Paths, exe: &Path, keep: &[Keep]) -> Result<(), String> {
    match lifecycle::stop(&paths.home) {
        Ok(Stopped::Exited(pid)) => println!("Stopped Sonara (pid {}).", pid.unwrap_or(0)),
        Ok(Stopped::NotRunning) => {}
        // A runtime that does not stop keeps its files open: remove nothing.
        Err(e) => return Err(format!("Sonara did not stop ({e}); nothing was removed")),
    }
    let r = uninstall::remove_all(&paths.home, paths.runtime_root.as_deref(), exe, keep);
    let (keys, key_failures) = uninstall::remove_credentials(&uninstall::WindowsCredentials, keep);
    for id in &keys {
        println!("Removed the key of engine {id} from Credential Manager");
    }
    for (id, e) in &key_failures {
        println!("Could not remove the key of engine {id} ({e})");
    }
    for p in &r.removed {
        println!("Removed {}", p.display());
    }
    if let Some(d) = &r.deferred {
        println!(
            "Removed {} (finishes a few seconds after this command)",
            d.display()
        );
    }
    for p in &r.kept {
        println!("Kept {}", p.display());
    }
    for (p, e) in &r.failed {
        println!("Could not remove {} ({e})", p.display());
    }
    println!(
        "Sonara stays off: the plugin's hooks do nothing until /sonara:start. \
         To remove the plugin itself: /plugin uninstall sonara@sonara"
    );
    if r.failed.is_empty() {
        Ok(())
    } else {
        Err("some files could not be removed".into())
    }
}
