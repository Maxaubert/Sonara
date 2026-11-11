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
/// The runtime's file name next to `sonara-hook` and `sonara`
/// (`sonarad.exe` on Windows).
#[cfg(windows)]
pub const RUNTIME_EXE: &str = "sonarad.exe";
/// The runtime's file name next to `sonara-hook` and `sonara`
/// (`sonarad.exe` on Windows).
#[cfg(not(windows))]
pub const RUNTIME_EXE: &str = "sonarad";

/// Whether this platform can tell who holds a port (`peer_pid`,
/// `listener_pid`). Where it cannot, a pid in `runtime.json` is not checked.
const OWNER_CHECK: bool = cfg!(any(windows, target_os = "macos"));

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
/// pid (sonarad always writes one) is not checked. On Windows the check is
/// the owner of this very connection in the TCP table; on macOS it is a
/// socket listening on 127.0.0.1 at the port in the named process (only one
/// socket can listen there, and the connection is not accepted yet).
pub fn connect(rt: &Runtime, timeout: Duration) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rt.port));
    let s = TcpStream::connect_timeout(&addr, timeout)?;
    if let Some(pid) = rt.pid.filter(|_| OWNER_CHECK) {
        #[cfg(windows)]
        let owner = peer_pid(rt.port, s.local_addr()?.port());
        #[cfg(not(windows))]
        let owner = u32::try_from(pid)
            .ok()
            .and_then(|p| listener_pid(p, rt.port));
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

/// `pid` itself when it holds a socket listening on 127.0.0.1 at
/// `server_port`, from its file table (libproc). A process of another user
/// cannot be read and is no match. The offsets are those of
/// `<sys/proc_info.h>` (`struct socket_fdinfo`, 792 bytes, the same on
/// arm64 and x86_64).
#[cfg(target_os = "macos")]
fn listener_pid(pid: u32, server_port: u16) -> Option<u32> {
    use core::ffi::{c_int, c_void};
    extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
        fn proc_pidfdinfo(
            pid: c_int,
            fd: c_int,
            flavor: c_int,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }
    const PROC_PIDLISTFDS: c_int = 1;
    const PROC_PIDFDSOCKETINFO: c_int = 3;
    const PROX_FDTYPE_SOCKET: u32 = 2;
    const SOCKET_FDINFO: usize = 792;
    const FAMILY: usize = 184; // psi.soi_family
    const KIND: usize = 256; // psi.soi_kind
    const LPORT: usize = 268; // psi.soi_proto.pri_tcp.tcpsi_ini.insi_lport
    const LADDR4: usize = 324; // ...insi_laddr.ina_46.i46a_addr4
    const STATE: usize = 344; // psi.soi_proto.pri_tcp.tcpsi_state
    const AF_INET: i32 = 2;
    const SOCKINFO_TCP: i32 = 2;
    const TSI_S_LISTEN: i32 = 1;
    let pid_c = c_int::try_from(pid).ok().filter(|p| *p > 0)?;
    let i32_at = |b: &[u8], at: usize| i32::from_ne_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    // The fd list (struct proc_fdinfo: fd, type); read again if it filled
    // the buffer (the process opened files meanwhile).
    let mut fds: Vec<[u32; 2]> = Vec::new();
    for _ in 0..4 {
        // SAFETY: a null buffer asks for the size only.
        let need = unsafe { proc_pidinfo(pid_c, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if need <= 0 {
            return None;
        }
        let cap = need as usize / 8 + 16;
        fds = vec![[0u32; 2]; cap];
        let bytes = c_int::try_from(cap * 8).ok()?;
        // SAFETY: `fds` holds `bytes` bytes; the call writes no more.
        let got =
            unsafe { proc_pidinfo(pid_c, PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), bytes) };
        if got <= 0 {
            return None;
        }
        let n = got as usize / 8;
        fds.truncate(n);
        if n < cap {
            break;
        }
    }
    let mut info = vec![0u64; SOCKET_FDINFO / 8];
    for [fd, kind] in fds {
        if kind != PROX_FDTYPE_SOCKET {
            continue;
        }
        // SAFETY: `info` holds SOCKET_FDINFO bytes; the call writes no more.
        let got = unsafe {
            proc_pidfdinfo(
                pid_c,
                fd as c_int,
                PROC_PIDFDSOCKETINFO,
                info.as_mut_ptr().cast(),
                SOCKET_FDINFO as c_int,
            )
        };
        if usize::try_from(got).ok() != Some(SOCKET_FDINFO) {
            continue;
        }
        // SAFETY: the u64 buffer of SOCKET_FDINFO bytes, read as bytes.
        let b = unsafe { std::slice::from_raw_parts(info.as_ptr().cast::<u8>(), SOCKET_FDINFO) };
        // insi_lport holds the port in network byte order.
        let port = u16::from_be(i32_at(b, LPORT) as u16);
        if i32_at(b, FAMILY) == AF_INET
            && i32_at(b, KIND) == SOCKINFO_TCP
            && i32_at(b, STATE) == TSI_S_LISTEN
            && port == server_port
            && b[LADDR4..LADDR4 + 4] == [127, 0, 0, 1]
        {
            return Some(pid);
        }
    }
    None
}

/// Neither Windows nor macOS: never asked (`OWNER_CHECK`).
#[cfg(not(any(windows, target_os = "macos")))]
fn listener_pid(_pid: u32, _server_port: u16) -> Option<u32> {
    None
}

/// `RUNTIME_EXE` next to the running program `me`, if it is there.
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
    spawn_detached(exe, args)
}

/// Run the child in a new session (`setsid` before exec): no controlling
/// terminal, and out of the caller's process group, which Claude Code may
/// kill with the hook (the runtime would go with it and every hook would
/// start a new one).
#[cfg(unix)]
fn new_session(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn setsid() -> i32;
    }
    // SAFETY: the closure runs in the child between fork and exec and only
    // calls setsid, which is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Start `exe` with `args` on its own: no window, no inherited handles, out
/// of the caller's job when Windows allows it (in a new session on unix),
/// so it outlives the caller and the caller's host never waits for it.
pub fn spawn_detached(exe: &Path, args: &[String]) -> std::io::Result<Child> {
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
        #[cfg(unix)]
        new_session(&mut cmd);
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
    #[cfg(any(windows, target_os = "macos"))]
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

    // The hook's process group can be killed with the hook; a runtime it
    // starts must not be in it, or every hook would start a new runtime.
    #[cfg(unix)]
    #[test]
    fn a_detached_child_leads_its_own_session() {
        extern "C" {
            fn getsid(pid: i32) -> i32;
            fn getpgid(pid: i32) -> i32;
        }
        let mut child = spawn_detached(Path::new("/bin/sleep"), &["5".to_string()]).unwrap();
        let pid = child.id() as i32;
        // SAFETY: plain queries of a pid.
        let (sid, pgid, mine) = unsafe { (getsid(pid), getpgid(pid), getsid(0)) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(sid, pid, "the child leads a new session");
        assert_eq!(pgid, pid, "and its own process group");
        assert_ne!(sid, mine);
    }

    #[test]
    fn the_runtime_exe_has_the_platform_suffix() {
        assert_eq!(
            RUNTIME_EXE,
            format!("sonarad{}", std::env::consts::EXE_SUFFIX)
        );
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
