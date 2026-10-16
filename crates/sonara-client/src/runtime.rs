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

/// Connect to the runtime. Before anything is sent, the connection's other
/// end must belong to the process `runtime.json` names (#56): after a crash
/// the file names a free port, and another process (of any local user) that
/// listens on it would get the token and the text. A squatter is
/// `PermissionDenied`, which callers treat as no runtime. A file without a
/// pid (sonarad always writes one) is not checked.
pub fn connect(rt: &Runtime, timeout: Duration) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rt.port));
    let s = TcpStream::connect_timeout(&addr, timeout)?;
    if let Some(pid) = rt.pid.filter(|_| cfg!(windows)) {
        let owner = peer_pid(rt.port, s.local_addr()?.port());
        if owner.map(u64::from) != Some(pid) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "port {} is not held by the runtime (pid {pid}, holder {owner:?})",
                    rt.port
                ),
            ));
        }
    }
    Ok(s)
}

/// The pid that owns the loopback connection from `client_port` to
/// `server_port` on the server side, from the system's TCP table.
#[cfg(windows)]
fn peer_pid(server_port: u16, client_port: u16) -> Option<u32> {
    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(
            table: *mut core::ffi::c_void,
            size: *mut u32,
            order: i32,
            family: u32,
            class: i32,
            reserved: u32,
        ) -> u32;
    }
    const AF_INET: u32 = 2;
    const TCP_TABLE_OWNER_PID_CONNECTIONS: i32 = 4;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    // 127.0.0.1 and a port as the table stores them (network byte order).
    const LOOPBACK: u32 = u32::from_ne_bytes([127, 0, 0, 1]);
    let port = |raw: u32| u16::from_be(raw as u16);
    let mut buf: Vec<u32> = Vec::new();
    let mut size = 0u32;
    // The table can grow between the size query and the read: retry.
    for _ in 0..4 {
        let ptr = if buf.is_empty() {
            std::ptr::null_mut()
        } else {
            buf.as_mut_ptr().cast()
        };
        // SAFETY: `ptr` is null with `size` 0, or `buf` of at least `size`
        // bytes; the call writes no more than `size` bytes.
        let r = unsafe {
            GetExtendedTcpTable(
                ptr,
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        match r {
            0 if !buf.is_empty() => {
                // MIB_TCPTABLE_OWNER_PID: the row count, then rows of
                // state, local address, local port, remote address,
                // remote port, owning pid.
                let n = buf[0] as usize;
                return buf[1..]
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .take(n)
                    .find(|row| {
                        row[1] == LOOPBACK
                            && port(row[2]) == server_port
                            && row[3] == LOOPBACK
                            && port(row[4]) == client_port
                    })
                    .map(|row| row[5]);
            }
            0 | ERROR_INSUFFICIENT_BUFFER => {
                buf = vec![0u32; (size as usize).div_ceil(4) + 6];
                size = (buf.len() * 4) as u32;
            }
            _ => return None,
        }
    }
    None
}

/// Not Windows: never asked (the runtime is Windows only).
#[cfg(not(windows))]
fn peer_pid(_server_port: u16, _client_port: u16) -> Option<u32> {
    None
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

    // #56: after a crash, `runtime.json` names a free port; a process of
    // another user that listens on it got the token and the turn's text.
    // A port held by a process other than the one named is not the runtime.
    #[cfg(windows)]
    #[test]
    fn a_port_held_by_another_process_is_not_the_runtime() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let rt = |pid| Runtime {
            port,
            token: "t".into(),
            pid,
        };
        let me = u64::from(std::process::id());
        assert!(connect(&rt(Some(me)), PROBE).is_ok(), "the named process");
        let err = connect(&rt(Some(me + 4)), PROBE).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        // A file without a pid is not one sonarad writes, but cannot be
        // checked either.
        assert!(connect(&rt(None), PROBE).is_ok());
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
