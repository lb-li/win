use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use fsearch_core::collector::change::{classify, Change};
use fsearch_core::collector::usn_parse::parse_usn_events;
use fsearch_core::collector::CollectError;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ffi::{
    create_size, journal_size, read_size, CreateUsnJournalData, ReadUsnJournalData, UsnJournalData,
    FSCTL_CREATE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, GENERIC_READ,
};

const BUF_BYTES: usize = 1 << 18;

pub struct UsnWatcher {
    handle: HANDLE,
    journal_id: u64,
    next_usn: i64,
}

fn wide(path: &str) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn open_handle(letter: char) -> Result<HANDLE, CollectError> {
    let path = format!("\\\\.\\{}:", letter.to_ascii_uppercase());
    let handle = CreateFileW(
        wide(&path).as_ptr(),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        std::ptr::null(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        std::ptr::null_mut(),
    );
    if handle == INVALID_HANDLE_VALUE {
        return Err(CollectError::Inaccessible(letter));
    }
    Ok(handle)
}

unsafe fn query_journal(handle: HANDLE) -> Result<UsnJournalData, CollectError> {
    let mut data: UsnJournalData = std::mem::zeroed();
    let mut returned = 0u32;
    let ok = DeviceIoControl(
        handle,
        FSCTL_QUERY_USN_JOURNAL,
        std::ptr::null(),
        0,
        &mut data as *mut _ as *mut _,
        journal_size(),
        &mut returned,
        std::ptr::null_mut(),
    );
    if ok == 0 {
        return Err(CollectError::Ioctl("FSCTL_QUERY_USN_JOURNAL".into()));
    }
    Ok(data)
}

unsafe fn ensure_journal(handle: HANDLE) -> Result<UsnJournalData, CollectError> {
    match query_journal(handle) {
        Ok(journal) => Ok(journal),
        Err(_) => {
            let create = CreateUsnJournalData {
                maximum_size: 64 << 20,
                allocation_delta: 8 << 20,
            };
            let mut returned = 0u32;
            let created = DeviceIoControl(
                handle,
                FSCTL_CREATE_USN_JOURNAL,
                &create as *const _ as *const _,
                create_size(),
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            );
            if created == 0 {
                return Err(CollectError::Ioctl("FSCTL_CREATE_USN_JOURNAL".into()));
            }
            query_journal(handle)
        }
    }
}

impl UsnWatcher {
    pub fn open(letter: char) -> Result<Self, CollectError> {
        unsafe {
            let handle = open_handle(letter)?;
            let journal = match ensure_journal(handle) {
                Ok(journal) => journal,
                Err(err) => {
                    CloseHandle(handle);
                    return Err(err);
                }
            };
            Ok(Self {
                handle,
                journal_id: journal.usn_journal_id,
                next_usn: journal.next_usn,
            })
        }
    }

    pub fn journal_id(&self) -> u64 {
        self.journal_id
    }

    pub fn next_usn(&self) -> i64 {
        self.next_usn
    }

    pub fn ensure_current(&mut self) -> Result<(), CollectError> {
        let journal = unsafe { query_journal(self.handle)? };
        if journal.usn_journal_id != self.journal_id {
            return Err(CollectError::Ioctl("journal recreated".into()));
        }
        Ok(())
    }

    pub fn read_batch(&mut self, timeout_ms: u64) -> Result<Vec<Change>, CollectError> {
        let input = ReadUsnJournalData {
            start_usn: self.next_usn,
            reason_mask: u32::MAX,
            return_only_on_close: 0,
            timeout: timeout_ms,
            bytes_to_wait_for: 0,
            min_major_version: 2,
            max_major_version: 2,
        };
        let mut buffer = vec![0u8; BUF_BYTES];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                FSCTL_READ_USN_JOURNAL,
                &input as *const _ as *const _,
                read_size(),
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(CollectError::Ioctl("FSCTL_READ_USN_JOURNAL".into()));
        }

        let (events, next) = parse_usn_events(&buffer[..returned as usize]);
        if next > 0 {
            self.next_usn = next as i64;
        }
        Ok(events.into_iter().filter_map(classify).collect())
    }
}

impl Drop for UsnWatcher {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
