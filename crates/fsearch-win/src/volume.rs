use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW, DRIVE_FIXED,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub letter: char,
    pub serial: u32,
    pub fs: String,
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

pub fn list_volumes() -> Vec<Volume> {
    let mut out = Vec::new();
    let needed = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
    if needed == 0 {
        return out;
    }
    let mut buf = vec![0u16; needed as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };
    if written == 0 {
        return out;
    }
    buf.truncate(written as usize);

    for chunk in buf.split(|&c| c == 0) {
        if chunk.len() < 3 {
            continue;
        }
        let root = OsString::from_wide(chunk);
        let text = root.to_string_lossy();
        let letter = text.chars().next().unwrap_or('?');
        if letter == '?' {
            continue;
        }
        let drive_type = unsafe { GetDriveTypeW(chunk.as_ptr()) };
        if drive_type != DRIVE_FIXED {
            continue;
        }
        let mut serial = 0u32;
        let mut fs_buf = vec![0u16; 32];
        let ok = unsafe {
            GetVolumeInformationW(
                chunk.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut serial,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            )
        };
        if ok == 0 {
            continue;
        }
        let fs = OsString::from_wide(&fs_buf)
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();
        if fs.eq_ignore_ascii_case("NTFS") {
            out.push(Volume { letter, serial, fs });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_check_returns_bool() {
        let _ = is_elevated();
    }

    #[test]
    fn volume_letters_are_uppercase_ascii() {
        for v in list_volumes() {
            assert!(v.letter.is_ascii_uppercase());
            assert!(v.fs.eq_ignore_ascii_case("NTFS"));
        }
    }
}
