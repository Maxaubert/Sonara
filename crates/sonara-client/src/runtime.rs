//! The running runtime: `runtime.json` read back, a connection to it, and
//! a new runtime started detached.
use crate::home::RUNTIME_FILE;
use serde_json::Value;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// How long a connection attempt to a runtime named in `runtime.json` may
/// take (a live runtime on loopback answers at once; a dead one's port can
/// take seconds to refuse on Windows).
pub const PROBE: Duration = Duration::from_millis(300);
/// The runtime's file name next to `sonara-hook.exe` and `sonara.exe`.
pub const RUNTIME_EXE: &str = "sonarad.exe";

/// Where and how to reach the runtime (from `runtime.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub port: u16,
    pub token: String,
    pub pid: Option<u64>,
}

/// The runtime named in the home's `runtime.json`, if the file parses.
pub fn read_runtime(home: &Path) -> Option<Runtime> {
    let raw = std::fs::read(home.join(RUNTIME_FILE)).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    Some(Runtime {
        port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
        token: v.get("token")?.as_str()?.to_string(),
        pid: v.get("pid").and_then(Value::as_u64),
    })
}

/// Connect to the runtime.
pub fn connect(rt: &Runtime, timeout: Duration) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rt.port));
    TcpStream::connect_timeout(&addr, timeout)
}

/// `sonarad.exe` next to the running program `me`, if it is there.
pub fn runtime_exe(me: &Path) -> Option<PathBuf> {
    let exe = me.parent()?.join(RUNTIME_EXE);
    exe.is_file().then_some(exe)
}

/// The extra runtime arguments of `SONARA_RUNTIME_ARGS` (split on
/// whitespace).
pub fn runtime_args(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    env("SONARA_RUNTIME_ARGS")
        .map(|a| a.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Keep the caller's standard handles out of the runtime it starts.
/// Windows gives a child every inheritable handle of its parent, and a
/// hook's stdin, stdout and stderr are Claude Code's pipes: a runtime
/// holding them would keep Claude Code waiting for the hook's output to
/// end for as long as the runtime lives.
#[cfg(windows)]
fn keep_std_handles_to_self() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(handle: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    let handles = [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ];
    for h in handles {
        if !h.is_null() && h as isize != -1 {
            // SAFETY: a handle of this process; clearing its inherit flag
            // changes nothing else (a failure leaves it as it was).
            unsafe {
                SetHandleInformation(h.cast(), HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(not(windows))]
fn keep_std_handles_to_self() {}

/// Start the runtime detached, with no window and none of the caller's
/// handles, out of Claude Code's job when the job allows it (so it
/// outlives the caller).
pub fn start_runtime(exe: &Path, args: &[String]) -> std::io::Result<Child> {
    keep_std_handles_to_self();
    let spawn = |flags: u32| {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(flags);
        }
        #[cfg(not(windows))]
        let _ = flags;
        cmd.spawn()
    };
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_file_is_read_back_or_ignored() {
        let dir = std::env::temp_dir().join(format!("sonara-client-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_runtime(&dir), None);
        std::fs::write(dir.join(RUNTIME_FILE), r#"{"port": 5000, "token": "abc"}"#).unwrap();
        assert_eq!(
            read_runtime(&dir),
            Some(Runtime {
                port: 5000,
                token: "abc".into(),
                pid: None,
            })
        );
        std::fs::write(dir.join(RUNTIME_FILE), "{not json").unwrap();
        assert_eq!(read_runtime(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_args_and_exe() {
        let env = |k: &str| (k == "SONARA_RUNTIME_ARGS").then(|| " --engine  fake ".to_string());
        assert_eq!(runtime_args(&env), ["--engine", "fake"]);
        assert!(runtime_args(&|_| None).is_empty());
        let dir = std::env::temp_dir().join(format!("sonara-client-exe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hook = dir.join("sonara-hook.exe");
        assert_eq!(runtime_exe(&hook), None, "no runtime next to it");
        std::fs::write(dir.join(RUNTIME_EXE), b"").unwrap();
        assert_eq!(runtime_exe(&hook), Some(dir.join(RUNTIME_EXE)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
