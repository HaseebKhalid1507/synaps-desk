//! The card-choices file on Windows, this user's only ([`super::create_private`],
//! Shady #93 P2-3): created with a protected DACL of one entry, the
//! current user's SID with full access. Protected, nothing is inherited
//! from the folder, however open the folder is; the rename that puts it in
//! place keeps it. What can't be set fails the write: nothing is written.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::FromRawHandle as _;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `s`, NUL-ended UTF-16.
pub(crate) fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// A NUL-ended UTF-16 string the system allocated, read and freed.
///
/// # Safety
/// `s` is a valid NUL-ended string from `LocalAlloc`.
pub(crate) unsafe fn take(s: *mut u16) -> String {
    let n = (0..).take_while(|&i| unsafe { *s.add(i) } != 0).count();
    let out = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(s, n) });
    unsafe { LocalFree(s.cast()) };
    out
}

/// The current user's SID (`S-1-5-21-…`), from the process's token.
fn user_sid() -> io::Result<String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
        // u64s: TOKEN_USER's alignment.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        let err = io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(err);
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(take(s))
    }
}

/// The file's security descriptor, as SDDL: a protected DACL (`P`), one
/// entry allowing full file access (`FA`) to the current user.
fn private_sddl() -> io::Result<String> {
    Ok(format!("D:P(A;;FA;;;{})", user_sid()?))
}

/// A security descriptor made from `sddl`, freed when dropped.
pub(crate) struct Descriptor(pub(crate) PSECURITY_DESCRIPTOR);

impl Descriptor {
    pub(crate) fn of(sddl: &str) -> io::Result<Self> {
        let s = wide(OsStr::new(sddl));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(s.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(sd))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// A new file at `path` (never an existing one), created with the
/// private descriptor, open to write. Shared as std's files are, so the
/// rename that follows may move it.
pub(crate) fn create_private(path: &Path) -> io::Result<std::fs::File> {
    let sd = Descriptor::of(&private_sddl()?)?;
    let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
    let name = wide(path.as_os_str());
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &sa,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_handle(h) })
}

/// Tests: an object's DACL read back, and a folder's set; this user's
/// SID as the test finds it (its own token, not [`user_sid`]), and
/// whether a DACL allows that SID alone (Shady #93 R2 P3-1).
#[cfg(test)]
pub(crate) mod acl {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    /// `ACCESS_ALLOWED_ACE_TYPE` (`Win32_System_SystemServices`, a feature
    /// the desk doesn't take for one number).
    const ACCESS_ALLOWED: u8 = 0;

    /// A SID: its bytes, and its string (`S-1-…`).
    pub(crate) struct Sid {
        bytes: Vec<u64>,
        pub(crate) text: String,
    }

    impl Sid {
        pub(crate) fn psid(&self) -> PSID {
            self.bytes.as_ptr() as PSID
        }

        /// The SID whose string is `text` (`S-1-…`).
        pub(crate) fn parse(text: &str) -> io::Result<Sid> {
            let s = wide(OsStr::new(text));
            let mut sid: PSID = null_mut();
            if unsafe { ConvertStringSidToSidW(s.as_ptr(), &mut sid) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let out = unsafe { Sid::copy(sid) };
            unsafe { LocalFree(sid) };
            out
        }

        /// The user this test process runs as, from its own token:
        /// `TokenUser`, its SID's string by `ConvertSidToStringSidW`.
        pub(crate) fn of_this_process() -> io::Result<Sid> {
            unsafe {
                let mut token: HANDLE = null_mut();
                if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut len = 0u32;
                GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
                let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
                let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
                let err = io::Error::last_os_error();
                CloseHandle(token);
                if ok == 0 {
                    return Err(err);
                }
                Sid::copy((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid)
            }
        }

        /// # Safety
        /// `sid` is a valid SID.
        unsafe fn copy(sid: PSID) -> io::Result<Sid> {
            let mut s: *mut u16 = null_mut();
            if unsafe { ConvertSidToStringSidW(sid, &mut s) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let text = unsafe { take(s) };
            let len = unsafe { GetLengthSid(sid) } as usize;
            let mut bytes = vec![0u64; len.div_ceil(8)];
            unsafe { std::ptr::copy_nonoverlapping(sid as *const u8, bytes.as_mut_ptr().cast::<u8>(), len) };
            Ok(Sid { bytes, text })
        }
    }

    /// A descriptor's DACL as SDDL after a round trip through Windows: the
    /// form Windows writes a DACL back in (a SID or its alias).
    pub(crate) fn canonical(sddl: &str) -> io::Result<String> {
        sddl_of(Descriptor::of(sddl)?.0)
    }

    /// Whether `sd`'s DACL is protected and holds one entry, allowing full
    /// file access to `sid`, compared as SIDs.
    pub(crate) fn allows_only(sd: PSECURITY_DESCRIPTOR, sid: &Sid) -> io::Result<bool> {
        let (mut present, mut defaulted) = (0, 0);
        let mut acl: *mut ACL = null_mut();
        if unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut control, mut revision) = (0u16, 0u32);
        if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || acl.is_null() || control & SE_DACL_PROTECTED == 0 || unsafe { (*acl).AceCount } != 1 {
            return Ok(false);
        }
        let mut ace: *mut core::ffi::c_void = null_mut();
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        let trustee = &ace.SidStart as *const u32 as PSID;
        Ok(ace.Header.AceType == ACCESS_ALLOWED && ace.Mask == FILE_ALL_ACCESS && unsafe { EqualSid(trustee, sid.psid()) } != 0)
    }

    /// The security descriptor (its DACL) of the file or folder at `path`.
    pub(crate) fn descriptor(path: &Path) -> io::Result<Descriptor> {
        let name = wide(path.as_os_str());
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let e = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        Ok(Descriptor(sd))
    }

    /// `sd`'s DACL as SDDL (`D:<flags>(<ace>)…`).
    pub(crate) fn sddl_of(sd: PSECURITY_DESCRIPTOR) -> io::Result<String> {
        let mut s: *mut u16 = null_mut();
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut s, null_mut())
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { take(s) })
    }

    /// The DACL of the file or folder at `path`, as SDDL.
    pub(crate) fn dacl(path: &Path) -> io::Result<String> {
        sddl_of(descriptor(path)?.0)
    }

    /// Set the folder at `path`'s DACL to `sddl`'s, protected.
    pub(crate) fn set_dacl(path: &Path, sddl: &str) -> io::Result<()> {
        let sd = Descriptor::of(sddl)?;
        let (mut present, mut defaulted) = (0, 0);
        let mut acl: *mut ACL = null_mut();
        if unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut acl, &mut defaulted) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let name = wide(path.as_os_str());
        let e = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null_mut(),
            )
        };
        if e != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(e as i32));
        }
        Ok(())
    }
}
