//! Daemon IPC endpoint: a Unix domain socket, or on Windows a named pipe whose
//! name is derived from the endpoint path.
//!
//! The Windows half (`pipe_name`, `endpoint_key` and the same-user server
//! check) is ported from Synaps `crates/agent-engine/src/ipc.rs`
//! (Copyright the Synaps authors, Apache-2.0), so this client and the daemon
//! derive the same pipe name from the same `daemon.json` socket path.

use std::io;
use std::path::Path;

pub use imp::*;

#[cfg(unix)]
mod imp {
    use super::*;

    pub type ClientStream = tokio::net::UnixStream;
    pub type ReadHalf = tokio::io::ReadHalf<ClientStream>;
    pub type WriteHalf = tokio::io::WriteHalf<ClientStream>;

    /// Connect and check that the server runs as this user, before a byte
    /// (the `Hello` with its cwd, then prompts) is written: the analogue of
    /// the Windows `verify_server_is_current_user`, and of the daemon's own
    /// `peer_cred().uid() == geteuid()` check on its clients. The run dir's
    /// 0700 alone does not hold with `SYNAPS_RUNTIME_DIR` pointed at a
    /// shared or pre-created directory.
    pub async fn connect(path: &Path) -> io::Result<ClientStream> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let server = stream.peer_cred()?.uid();
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        if server != me {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("the daemon socket {} is served by uid {server}, not this user (uid {me}); refusing to connect", path.display()),
            ));
        }
        Ok(stream)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    pub type ClientStream = NamedPipeClient;
    pub type ReadHalf = tokio::io::ReadHalf<ClientStream>;
    pub type WriteHalf = tokio::io::WriteHalf<ClientStream>;

    /// All pipe instances busy: retry this long (one is between an accept
    /// and the next instance's creation).
    const BUSY_RETRY: Duration = Duration::from_secs(2);

    /// Open the daemon's pipe and check that its server runs as this user.
    pub async fn connect(path: &Path) -> io::Result<ClientStream> {
        use std::os::windows::io::AsRawHandle;
        let name = pipe_name(path);
        let deadline = Instant::now() + BUSY_RETRY;
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(e) => return Err(e),
            }
        };
        win::verify_server_is_current_user(client.as_raw_handle())?;
        Ok(client)
    }

    mod win {
        use std::io;
        use std::ptr::null_mut;

        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Security::{EqualSid, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser};
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

        struct Handle(HANDLE);
        impl Drop for Handle {
            fn drop(&mut self) {
                // SAFETY: a handle we opened and own.
                unsafe { CloseHandle(self.0) };
            }
        }

        /// A process token's `TOKEN_USER` (8-aligned: it holds pointers).
        struct TokenUserBuf(Vec<u64>);

        impl TokenUserBuf {
            fn of(process: HANDLE) -> io::Result<Self> {
                let mut token: HANDLE = null_mut();
                // SAFETY: valid process handle and out-pointer.
                if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                let token = Handle(token);
                let mut len = 0u32;
                // SAFETY: size query (fails with ERROR_INSUFFICIENT_BUFFER, sets len).
                unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len) };
                if len == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut buf = vec![0u64; (len as usize).div_ceil(8)];
                // SAFETY: buf holds at least `len` bytes.
                if unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self(buf))
            }

            fn current() -> io::Result<Self> {
                // SAFETY: the pseudo-handle needs no close.
                Self::of(unsafe { GetCurrentProcess() })
            }

            fn sid(&self) -> PSID {
                // SAFETY: filled by GetTokenInformation(TokenUser).
                unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
            }
        }

        /// Refuse a pipe whose server process runs as another account.
        pub fn verify_server_is_current_user(pipe: std::os::windows::io::RawHandle) -> io::Result<()> {
            let mut pid = 0u32;
            // SAFETY: a connected client pipe handle.
            if unsafe { GetNamedPipeServerProcessId(pipe as HANDLE, &mut pid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let refuse = |why: String| {
                io::Error::new(io::ErrorKind::PermissionDenied, format!("daemon pipe is served by pid {pid}, {why}; refusing to connect"))
            };
            // SAFETY: plain OpenProcess; null on failure.
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if process.is_null() {
                return Err(refuse(format!("whose owner cannot be read ({})", io::Error::last_os_error())));
            }
            let process = Handle(process);
            let theirs = TokenUserBuf::of(process.0).map_err(|e| refuse(format!("whose owner cannot be read ({e})")))?;
            let ours = TokenUserBuf::current()?;
            // SAFETY: two valid SIDs.
            if unsafe { EqualSid(ours.sid(), theirs.sid()) } == 0 {
                return Err(refuse("which runs as another user".into()));
            }
            Ok(())
        }
    }
}

/// The Windows pipe name for an endpoint path (ported from Synaps
/// `ipc::pipe_name`). Hashed, because pipe names live in one flat namespace
/// and may not contain `\`. One spelling of the path is hashed: absolute,
/// directory canonicalised when it exists, no `\\?\` prefix, `\`
/// separators, lowercase.
#[cfg(windows)]
pub fn pipe_name(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let key = endpoint_key(path);
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(r"\\.\pipe\synaps-daemon-{hex}")
}

#[cfg(windows)]
fn endpoint_key(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let resolved = match (abs.parent(), abs.file_name()) {
        (Some(dir), Some(name)) => dir.canonicalize().map(|d| d.join(name)).unwrap_or_else(|_| abs.clone()),
        _ => abs,
    };
    let s = resolved.to_string_lossy().replace('/', "\\");
    let s = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    };
    s.to_lowercase()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_stable_and_case_and_separator_insensitive() {
        let a = pipe_name(Path::new(r"C:\Users\Me\.synaps-cli\run\daemon.sock"));
        let b = pipe_name(Path::new("c:/users/me/.synaps-cli/run/DAEMON.sock"));
        assert_eq!(a, b);
        assert!(a.starts_with(r"\\.\pipe\synaps-daemon-"));
        assert_ne!(a, pipe_name(Path::new(r"C:\Users\Me\.synaps-cli\run\daemon-work.sock")));
    }

    #[test]
    fn pipe_name_matches_the_daemon_derivation() {
        // sha256("c:\\users\\me\\.synaps-cli\\run\\daemon.sock")[..16], as Synaps computes it.
        use sha2::{Digest, Sha256};
        let key = r"c:\users\me\.synaps-cli\run\daemon.sock";
        let hex: String = Sha256::digest(key.as_bytes())[..16].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(pipe_name(Path::new(r"C:\Users\Me\.synaps-cli\run\daemon.sock")), format!(r"\\.\pipe\synaps-daemon-{hex}"));
    }

    #[tokio::test]
    async fn connect_without_a_server_is_an_error() {
        let p = std::env::temp_dir().join(format!("nobody-{}.sock", std::process::id()));
        assert!(connect(&p).await.is_err());
    }
}
