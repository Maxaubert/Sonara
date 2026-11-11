//! The built `sonarad` on unix (macOS): it starts, writes a user-only
//! `runtime.json` (mode 0600) next to its lock, a second runtime on the same
//! home exits with code 3, and SIGTERM ends it cleanly (exit 0,
//! `runtime.json` removed), as Ctrl+C does.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A temp home and the runtime on it; killed and removed when dropped.
struct Runtime {
    home: PathBuf,
    child: Child,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn sonarad(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sonarad"));
    c.arg("--home")
        .arg(home)
        .args(["--engine", "fake", "--system", "fake", "--keys", "fake"])
        .args(["--idle-exit", "60"])
        .env_remove("SONARA_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}

fn start(tag: &str) -> Runtime {
    let home = std::env::temp_dir().join(format!("sonarad-unix-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let child = sonarad(&home).spawn().expect("start sonarad");
    Runtime { home, child }
}

fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + TIMEOUT;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

fn written_by(rt: &Runtime) -> bool {
    let Ok(text) = std::fs::read_to_string(rt.home.join("runtime.json")) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .map(|v| v["pid"] == rt.child.id())
        .unwrap_or(false)
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn sonarad_starts_and_its_files_are_user_only() {
    let rt = start("files");
    assert!(
        wait_for(|| written_by(&rt)),
        "sonarad wrote no runtime.json"
    );
    assert_eq!(mode(&rt.home.join("runtime.json")), 0o600);
    assert_eq!(mode(&rt.home.join("sonarad.lock")), 0o600);
}

#[test]
fn a_second_runtime_on_the_same_home_exits_with_code_3() {
    let rt = start("second");
    assert!(
        wait_for(|| written_by(&rt)),
        "sonarad wrote no runtime.json"
    );
    let status = sonarad(&rt.home).status().expect("run a second sonarad");
    assert_eq!(status.code(), Some(3));
    assert!(written_by(&rt), "the first runtime's file stays");
}

#[test]
fn sigterm_ends_the_runtime_cleanly() {
    let mut rt = start("sigterm");
    assert!(
        wait_for(|| written_by(&rt)),
        "sonarad wrote no runtime.json"
    );
    // SAFETY: a signal to the child this test started.
    assert_eq!(
        unsafe { libc::kill(rt.child.id() as i32, libc::SIGTERM) },
        0
    );
    let mut status = None;
    assert!(wait_for(|| {
        status = rt.child.try_wait().unwrap();
        status.is_some()
    }));
    assert!(status.unwrap().success(), "{status:?}");
    assert!(!rt.home.join("runtime.json").exists());
}
