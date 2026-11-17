use crate::collector::RawRecord;

pub const USN_V2_HEADER_SIZE: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsnEvent {
    pub file_id: u64,
    pub parent_file_id: u64,
    pub name: String,
    pub attributes: u32,
    pub reason: u32,
}

pub fn utf16_name(buf: &[u8], offset: usize, byte_len: usize) -> String {
    let end = offset.saturating_add(byte_len).min(buf.len());
    if offset >= end {
        return String::new();
    }
    let units: Vec<u16> = buf[offset..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub fn parse_usn_records(bytes: &[u8]) -> (Vec<RawRecord>, u64) {
    let (events, next) = parse_usn_events(bytes);
    let records = events
        .into_iter()
        .map(|e| RawRecord {
            file_id: e.file_id,
            parent_file_id: e.parent_file_id,
            name: e.name,
            attributes: e.attributes,
        })
        .collect();
    (records, next)
}

pub fn parse_usn_events(bytes: &[u8]) -> (Vec<UsnEvent>, u64) {
    let mut events = Vec::new();
    let Some(payload_end) = bytes.len().checked_sub(8) else {
        return (events, 0);
    };
    let mut pos = 0usize;
    while pos + USN_V2_HEADER_SIZE <= payload_end {
        let record_len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        if record_len < USN_V2_HEADER_SIZE || pos + record_len > payload_end {
            break;
        }
        let rec = &bytes[pos..pos + record_len];
        let reason = u32::from_le_bytes(rec[40..44].try_into().unwrap());
        let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
        let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;
        events.push(UsnEvent {
            file_id: u64::from_le_bytes(rec[8..16].try_into().unwrap()),
            parent_file_id: u64::from_le_bytes(rec[16..24].try_into().unwrap()),
            name: utf16_name(rec, name_off, name_len),
            attributes: u32::from_le_bytes(rec[52..56].try_into().unwrap()),
            reason,
        });
        pos += record_len;
    }
    let next = u64::from_le_bytes(bytes[payload_end..payload_end + 8].try_into().unwrap());
    (events, next)
}

#[cfg(test)]
pub fn build_test_record(
    file_id: u64,
    parent: u64,
    name: &str,
    attributes: u32,
    reason: u32,
) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let name_bytes = units.len() * 2;
    let total = (USN_V2_HEADER_SIZE + name_bytes + 7) & !7;
    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&(total as u32).to_le_bytes());
    buf[4..6].copy_from_slice(&2u16.to_le_bytes());
    buf[8..16].copy_from_slice(&file_id.to_le_bytes());
    buf[16..24].copy_from_slice(&parent.to_le_bytes());
    buf[40..44].copy_from_slice(&reason.to_le_bytes());
    buf[52..56].copy_from_slice(&attributes.to_le_bytes());
    buf[56..58].copy_from_slice(&(name_bytes as u16).to_le_bytes());
    buf[58..60].copy_from_slice(&(USN_V2_HEADER_SIZE as u16).to_le_bytes());
    for (i, unit) in units.iter().enumerate() {
        let at = USN_V2_HEADER_SIZE + i * 2;
        buf[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    buf
}

#[cfg(test)]
pub fn stream_with_next(records: &[Vec<u8>], next: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    for r in records {
        buf.extend_from_slice(r);
    }
    buf.extend_from_slice(&next.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(file_id: u64, parent: u64, name: &str, attributes: u32) -> Vec<u8> {
        build_test_record(file_id, parent, name, attributes, 0)
    }

    #[test]
    fn parses_multiple_records_and_next_frn() {
        let buf = stream_with_next(
            &[record(10, 5, "Users", 0x10), record(11, 10, "a.txt", 0)],
            99,
        );
        let (records, next) = parse_usn_records(&buf);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].file_id, 10);
        assert_eq!(records[0].parent_file_id, 5);
        assert_eq!(records[1].name, "a.txt");
        assert_eq!(records[0].attributes, 0x10);
        assert_eq!(next, 99);
    }

    #[test]
    fn decodes_utf16_names() {
        let buf = stream_with_next(&[record(1, 5, "报告.TXT", 0)], 0);
        let (records, _) = parse_usn_records(&buf);
        assert_eq!(records[0].name, "报告.TXT");
    }

    #[test]
    fn empty_payload_yields_nothing() {
        let (records, next) = parse_usn_records(&0u64.to_le_bytes());
        assert!(records.is_empty());
        assert_eq!(next, 0);
    }

    #[test]
    fn truncated_record_is_ignored() {
        let mut buf = record(1, 5, "hello.txt", 0);
        buf.truncate(20);
        buf.extend_from_slice(&0u64.to_le_bytes());
        let (records, _) = parse_usn_records(&buf);
        assert!(records.is_empty());
    }
}
