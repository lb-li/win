use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use fsearch_core::collector::usn_parse::parse_usn_records;
use fsearch_core::collector::{CollectError, Collector, RecordBatch};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::ffi::{
    journal_size, mft_enum_size, MftEnumData, UsnJournalData, FSCTL_ENUM_USN_DATA,
    FSCTL_QUERY_USN_JOURNAL, GENERIC_READ,
};

const IOCTL_BUF: usize = 1 << 20;

pub struct MftScanner {
    pub letter: char,
}

impl MftScanner {
    pub fn new(letter: char) -> Self {
        Self { letter }
    }
}

fn wide(path: &str) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn open_volume(letter: char) -> Result<HANDLE, CollectError> {
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

impl Collector for MftScanner {
    fn scan(&self) -> Result<RecordBatch, CollectError> {
        unsafe {
            let handle = open_volume(self.letter)?;
            let journal = match query_journal(handle) {
                Ok(journal) => journal,
                Err(err) => {
                    CloseHandle(handle);
                    return Err(err);
                }
            };

            let mut buffer = vec![0u8; IOCTL_BUF];
            let mut start: u64 = 0;
            let mut all = Vec::with_capacity(1 << 20);

            loop {
                let input = MftEnumData {
                    start_file_reference_number: start,
                    low_usn: 0,
                    high_usn: i64::MAX,
                };
                let mut returned = 0u32;
                let ok = DeviceIoControl(
                    handle,
                    FSCTL_ENUM_USN_DATA,
                    &input as *const _ as *const _,
                    mft_enum_size(),
                    buffer.as_mut_ptr() as *mut _,
                    buffer.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                );
                if ok == 0 {
                    CloseHandle(handle);
                    return Err(CollectError::Ioctl("FSCTL_ENUM_USN_DATA".into()));
                }
                let (records, next) = parse_usn_records(&buffer[..returned as usize]);
                all.extend(records);
                if next == 0 || next == start {
                    break;
                }
                start = next;
            }

            CloseHandle(handle);
            Ok(RecordBatch {
                records: all,
                next_usn: journal.next_usn,
            })
        }
    }
}
