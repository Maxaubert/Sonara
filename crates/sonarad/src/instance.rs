//! One instance per user (spec section 3): a named mutex
//! `Local\Sonara-Runtime-<hash>`, the user's SID, the random token, and a
//! user-only ACL for `runtime.json`.
//!
//! The hash is FNV-1a 64 (fixed, so every runtime version computes the same
//! name) of the user's SID string. For a home other than the default
//! `%LOCALAPPDATA%\Sonara` it also covers the home's canonical path: an
//! instance is found through its home's `runtime.json`, so separate homes
//! (tests, portable bundles) are separate instances.

/// FNV-1a, 64 bit.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The mutex name for this user and, when not the default home, this home.
pub fn mutex_name(sid: &str, home_key: Option<&str>) -> String {
    let key = match home_key {
        None => sid.to_string(),
        Some(h) => format!("{sid}\n{h}"),
    };
    format!("Local\\Sonara-Runtime-{:016x}", fnv1a64(key.as_bytes()))
}

#[derive(Debug)]
pub enum AcquireError {
    /// Another instance holds the mutex.
    AlreadyRunning,
    Os(String),
}

#[cfg(windows)]
mod win {
    use super::AcquireError;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(p: &Path) -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Holds the single-instance mutex until dropped (or the process ends).
    pub struct Instance {
        handle: HANDLE,
    }

    // The handle is only closed on drop.
    unsafe impl Send for Instance {}
    unsafe impl Sync for Instance {}

    impl Drop for Instance {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    pub fn acquire(name: &str) -> Result<Instance, AcquireError> {
        let w = wide(name);
        unsafe {
            let handle = CreateMutexW(None, false, PCWSTR(w.as_ptr()))
                .map_err(|e| AcquireError::Os(format!("cannot create the mutex {name}: {e}")))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Err(AcquireError::AlreadyRunning);
            }
            Ok(Instance { handle })
        }
    }

    /// The current user's SID as text (`S-1-5-21-...`).
    pub fn user_sid() -> Result<String, String> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(|e| format!("OpenProcessToken: {e}"))?;
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            // u64 storage keeps TOKEN_USER aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let res = GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            );
            let _ = CloseHandle(token);
            res.map_err(|e| format!("GetTokenInformation: {e}"))?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text)
                .map_err(|e| format!("ConvertSidToStringSidW: {e}"))?;
            let sid = text.to_string().map_err(|e| e.to_string());
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            sid
        }
    }

    /// Give `path` a protected DACL that lets only `sid` in (full access).
    pub fn restrict_to_user(path: &Path, sid: &str) -> Result<(), String> {
        let sddl = wide(&format!("D:P(A;;FA;;;{sid})"));
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .map_err(|e| format!("building the ACL: {e}"))?;
            let mut present = false.into();
            let mut defaulted = false.into();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let got = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted);
            let result = match got {
                Err(e) => Err(format!("reading the ACL: {e}")),
                Ok(()) => {
                    let p = wide_path(path);
                    let err = SetNamedSecurityInfoW(
                        PCWSTR(p.as_ptr()),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        None,
                        None,
                        Some(dacl),
                        None,
                    );
                    if err.is_ok() {
                        Ok(())
                    } else {
                        Err(format!(
                            "setting the ACL of {}: error {}",
                            path.display(),
                            err.0
                        ))
                    }
                }
            };
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            result
        }
    }

    /// How many entries the DACL of `path` has, and whether it is
    /// protected from inheritance (for tests of `restrict_to_user`).
    pub fn dacl_summary(path: &Path) -> Result<(u16, bool), String> {
        use windows::Win32::Security::Authorization::GetNamedSecurityInfoW;
        use windows::Win32::Security::{GetSecurityDescriptorControl, SE_DACL_PROTECTED};
        let p = wide_path(path);
        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let err = GetNamedSecurityInfoW(
                PCWSTR(p.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                &mut sd,
            );
            if err.is_err() {
                return Err(format!("GetNamedSecurityInfoW: error {}", err.0));
            }
            let count = if dacl.is_null() { 0 } else { (*dacl).AceCount };
            let mut control = 0u16;
            let mut revision = 0u32;
            let _ = GetSecurityDescriptorControl(sd, &mut control, &mut revision);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            Ok((count, control & SE_DACL_PROTECTED.0 != 0))
        }
    }

    pub fn random_bytes(buf: &mut [u8]) -> Result<(), String> {
        unsafe {
            BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
                .ok()
                .map_err(|e| format!("BCryptGenRandom: {e}"))
        }
    }
}

#[cfg(windows)]
pub use win::{acquire, dacl_summary, random_bytes, restrict_to_user, user_sid, Instance};

#[cfg(not(windows))]
mod other {
    //! sonarad runs on Windows only (R4); these keep the crate building
    //! elsewhere and fail at start.
    use super::AcquireError;
    use std::path::Path;

    pub struct Instance;

    const MSG: &str = "sonarad runs on Windows only";

    pub fn acquire(_name: &str) -> Result<Instance, AcquireError> {
        Err(AcquireError::Os(MSG.into()))
    }

    pub fn user_sid() -> Result<String, String> {
        Err(MSG.into())
    }

    pub fn restrict_to_user(_path: &Path, _sid: &str) -> Result<(), String> {
        Err(MSG.into())
    }

    pub fn random_bytes(_buf: &mut [u8]) -> Result<(), String> {
        Err(MSG.into())
    }
}

#[cfg(not(windows))]
pub use other::{acquire, random_bytes, restrict_to_user, user_sid, Instance};

/// A fresh token: 32 random bytes as hex.
pub fn new_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    random_bytes(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_the_reference_values() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn the_name_is_per_user_and_per_non_default_home() {
        let a = mutex_name("S-1-5-21-1", None);
        assert!(a.starts_with("Local\\Sonara-Runtime-"));
        assert_eq!(a.len(), "Local\\Sonara-Runtime-".len() + 16);
        assert_ne!(a, mutex_name("S-1-5-21-2", None));
        assert_ne!(a, mutex_name("S-1-5-21-1", Some("c:\\x")));
        assert_eq!(a, mutex_name("S-1-5-21-1", None));
    }

    #[test]
    fn tokens_are_long_and_fresh() {
        let a = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, new_token().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn a_second_acquire_of_the_same_name_fails() {
        let name = format!("Local\\Sonara-Runtime-test-{}", std::process::id());
        let first = acquire(&name).unwrap();
        assert!(matches!(acquire(&name), Err(AcquireError::AlreadyRunning)));
        drop(first);
        acquire(&name).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn the_sid_looks_like_a_sid() {
        assert!(user_sid().unwrap().starts_with("S-1-"));
    }
}
