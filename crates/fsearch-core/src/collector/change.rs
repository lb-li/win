use crate::collector::reasons::*;
use crate::collector::usn_parse::UsnEvent;
use crate::collector::RawRecord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Created(RawRecord),
    Deleted(u64),
    RenameOld {
        file_id: u64,
        name: String,
    },
    RenameNew {
        file_id: u64,
        parent: u64,
        name: String,
        attributes: u32,
    },
    Attributes {
        file_id: u64,
        attributes: u32,
    },
}

pub fn classify(event: UsnEvent) -> Option<Change> {
    if event.reason & USN_REASON_FILE_CREATE != 0 {
        return Some(Change::Created(RawRecord {
            file_id: event.file_id,
            parent_file_id: event.parent_file_id,
            name: event.name,
            attributes: event.attributes,
        }));
    }
    if event.reason & USN_REASON_FILE_DELETE != 0 {
        return Some(Change::Deleted(event.file_id));
    }
    if event.reason & USN_REASON_RENAME_OLD_NAME != 0 {
        return Some(Change::RenameOld {
            file_id: event.file_id,
            name: event.name,
        });
    }
    if event.reason & USN_REASON_RENAME_NEW_NAME != 0 {
        return Some(Change::RenameNew {
            file_id: event.file_id,
            parent: event.parent_file_id,
            name: event.name,
            attributes: event.attributes,
        });
    }
    if event.reason & USN_REASON_BASIC_INFO_CHANGE != 0 {
        return Some(Change::Attributes {
            file_id: event.file_id,
            attributes: event.attributes,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::usn_parse::{build_test_record, parse_usn_events, stream_with_next};

    fn events(records: &[Vec<u8>]) -> Vec<UsnEvent> {
        parse_usn_events(&stream_with_next(records, 0)).0
    }

    #[test]
    fn create_event_becomes_created() {
        let e = events(&[build_test_record(10, 5, "a.txt", 0, USN_REASON_FILE_CREATE)]);
        assert!(matches!(classify(e[0].clone()), Some(Change::Created(r)) if r.file_id == 10));
    }

    #[test]
    fn delete_event_has_no_name() {
        let e = events(&[build_test_record(11, 5, "", 0, USN_REASON_FILE_DELETE)]);
        assert!(matches!(classify(e[0].clone()), Some(Change::Deleted(11))));
    }

    #[test]
    fn rename_events_are_distinguished() {
        let e = events(&[
            build_test_record(12, 5, "old.txt", 0, USN_REASON_RENAME_OLD_NAME),
            build_test_record(12, 5, "new.txt", 0, USN_REASON_RENAME_NEW_NAME),
        ]);
        assert!(matches!(
            classify(e[0].clone()),
            Some(Change::RenameOld { file_id: 12, .. })
        ));
        assert!(matches!(
            classify(e[1].clone()),
            Some(Change::RenameNew { file_id: 12, .. })
        ));
    }

    #[test]
    fn basic_info_change_becomes_attributes() {
        let e = events(&[build_test_record(
            13,
            5,
            "x.txt",
            0x10,
            USN_REASON_BASIC_INFO_CHANGE,
        )]);
        assert!(matches!(
            classify(e[0].clone()),
            Some(Change::Attributes { file_id: 13, .. })
        ));
    }

    #[test]
    fn unrelated_reason_is_ignored() {
        let e = events(&[build_test_record(14, 5, "y.txt", 0, 0x0000_4000)]);
        assert!(classify(e[0].clone()).is_none());
    }
}
