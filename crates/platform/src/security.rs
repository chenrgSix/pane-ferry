use std::{ffi::OsStr, io, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE},
    Security::{Authorization::*, *},
    System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

pub fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn user_sid(process: HANDLE) -> io::Result<Vec<u8>> {
    token_sid(process, TokenUser)
}

fn token_sid(process: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u8>> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        GetTokenInformation(token, class, ptr::null_mut(), 0, &mut length);
        // Word-aligned storage for TOKEN_USER and its SID.
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let ok = GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        );
        CloseHandle(token);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = if class == TokenOwner {
            (*(buffer.as_ptr() as *const TOKEN_OWNER)).Owner
        } else {
            (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid
        };
        Ok(std::slice::from_raw_parts(sid.cast::<u8>(), GetLengthSid(sid) as usize).to_vec())
    }
}

pub fn same_process_user(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let other = user_sid(process);
        CloseHandle(process);
        match (other, user_sid(GetCurrentProcess())) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

pub struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let sid = user_sid(GetCurrentProcess())?;
            let mut string = ptr::null_mut();
            if ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut string) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0;
            while *string.add(len) != 0 {
                len += 1;
            }
            let sid_string = String::from_utf16_lossy(std::slice::from_raw_parts(string, len));
            LocalFree(string.cast());
            let sddl = wide(format!("D:P(A;OICI;FA;;;{sid_string})"));
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }
    }
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub fn protect_directory(path: &Path) -> io::Result<()> {
    let descriptor = Descriptor::new()?;
    unsafe {
        let mut dacl = ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        if GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) == 0 {
            return Err(io::Error::last_os_error());
        }
        let status = SetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    check_private(path)
}

pub fn check_private(path: &Path) -> io::Result<()> {
    unsafe {
        let sid = user_sid(GetCurrentProcess())?;
        let mut owner = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut sd = ptr::null_mut();
        let status = GetNamedSecurityInfoW(
            wide(path).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut sd,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let _descriptor = Descriptor(sd);
        // An elevated token may create files owned by its default owner group.
        // Accept only the current token's user/default owner; every grant must
        // still name this user SID (never an Administrators/Everyone grant).
        let default_owner = token_sid(GetCurrentProcess(), TokenOwner)?;
        if owner.is_null()
            || dacl.is_null()
            || (EqualSid(owner, sid.as_ptr().cast_mut().cast()) == 0
                && EqualSid(owner, default_owner.as_ptr().cast_mut().cast()) == 0)
        {
            return Err(io::Error::other("invalid owner or DACL"));
        }
        for index in 0..(*dacl).AceCount {
            let mut ace = ptr::null_mut();
            if GetAce(dacl, index as u32, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = &*(ace as *const ACE_HEADER);
            // A private directory may only grant access to this account.
            if header.AceType != 0
            /* ACCESS_ALLOWED_ACE_TYPE */
            {
                return Err(io::Error::other("unexpected private ACL"));
            }
            let entry = &*(ace as *const ACCESS_ALLOWED_ACE);
            if EqualSid(
                (&entry.SidStart as *const u32).cast_mut().cast(),
                sid.as_ptr().cast_mut().cast(),
            ) == 0
            {
                return Err(io::Error::other("public private file"));
            }
        }
        Ok(())
    }
}
