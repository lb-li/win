pub const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00B3;
pub const FSCTL_CREATE_USN_JOURNAL: u32 = 0x0009_00E7;
pub const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
pub const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F7;

#[repr(C)]
pub struct MftEnumData {
    pub start_file_reference_number: u64,
    pub low_usn: i64,
    pub high_usn: i64,
}

#[repr(C)]
pub struct UsnJournalData {
    pub usn_journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
    pub lowest_valid_usn: i64,
    pub max_usn: i64,
    pub maximum_size: u64,
    pub allocation_delta: u64,
}

#[repr(C)]
pub struct CreateUsnJournalData {
    pub maximum_size: u64,
    pub allocation_delta: u64,
}

#[repr(C)]
pub struct ReadUsnJournalData {
    pub start_usn: i64,
    pub reason_mask: u32,
    pub return_only_on_close: u32,
    pub timeout: u64,
    pub bytes_to_wait_for: u32,
    pub min_major_version: u16,
    pub max_major_version: u16,
}

pub fn mft_enum_size() -> u32 {
    std::mem::size_of::<MftEnumData>() as u32
}

pub fn journal_size() -> u32 {
    std::mem::size_of::<UsnJournalData>() as u32
}

pub fn create_size() -> u32 {
    std::mem::size_of::<CreateUsnJournalData>() as u32
}

pub fn read_size() -> u32 {
    std::mem::size_of::<ReadUsnJournalData>() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_codes_are_stable() {
        assert_eq!(FSCTL_ENUM_USN_DATA, 0x900B3);
        assert_eq!(FSCTL_READ_USN_JOURNAL, 0x900BB);
        assert_eq!(FSCTL_CREATE_USN_JOURNAL, 0x900E7);
        assert_eq!(FSCTL_QUERY_USN_JOURNAL, 0x900F7);
    }

    #[test]
    fn journal_struct_is_56_bytes() {
        assert_eq!(std::mem::size_of::<UsnJournalData>(), 56);
    }
}
