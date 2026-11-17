use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use thiserror::Error;

use crate::index::snapshot::IndexSnapshot;

pub const MAGIC: [u8; 8] = *b"FSEARCH\0";
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 64;
pub const ENDIAN_MARK: u32 = u32::from_le_bytes(*b"LE12");

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("bad magic")]
    BadMagic,
    #[error("unsupported format version {0}")]
    BadVersion(u32),
    #[error("unsupported byte order")]
    BadEndian,
    #[error("length mismatch")]
    LengthMismatch,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub endian: u32,
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
    pub record_count: u64,
    pub names_len: u64,
    pub generation: u64,
}

impl SnapshotHeader {
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..8].copy_from_slice(&self.magic);
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.endian.to_le_bytes());
        buf[16..24].copy_from_slice(&self.volume_serial.to_le_bytes());
        buf[24..32].copy_from_slice(&self.journal_id.to_le_bytes());
        buf[32..40].copy_from_slice(&self.next_usn.to_le_bytes());
        buf[40..48].copy_from_slice(&self.record_count.to_le_bytes());
        buf[48..56].copy_from_slice(&self.names_len.to_le_bytes());
        buf[56..64].copy_from_slice(&self.generation.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self, SnapshotError> {
        if buf.len() < HEADER_SIZE {
            return Err(SnapshotError::LengthMismatch);
        }
        if buf[0..8] != MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(SnapshotError::BadVersion(version));
        }
        let endian = u32::from_le_bytes(buf[12..16].try_into().unwrap());
        if endian != ENDIAN_MARK {
            return Err(SnapshotError::BadEndian);
        }
        Ok(Self {
            magic: buf[0..8].try_into().unwrap(),
            version,
            endian,
            volume_serial: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            journal_id: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            next_usn: i64::from_le_bytes(buf[32..40].try_into().unwrap()),
            record_count: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
            names_len: u64::from_le_bytes(buf[48..56].try_into().unwrap()),
            generation: u64::from_le_bytes(buf[56..64].try_into().unwrap()),
        })
    }
}

pub fn header_for(snap: &IndexSnapshot) -> SnapshotHeader {
    SnapshotHeader {
        magic: MAGIC,
        version: FORMAT_VERSION,
        endian: ENDIAN_MARK,
        volume_serial: snap.volume_serial,
        journal_id: snap.journal_id,
        next_usn: snap.next_usn,
        record_count: snap.record_count() as u64,
        names_len: snap.names().len() as u64,
        generation: snap.generation,
    }
}

pub fn write_snapshot(path: &Path, snap: &IndexSnapshot) -> Result<(), SnapshotError> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&header_for(snap).encode())?;
        let records = snap.records();
        let bytes = unsafe {
            std::slice::from_raw_parts(records.as_ptr() as *const u8, records.len() * 16)
        };
        file.write_all(bytes)?;
        file.write_all(snap.names())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 77);
        b.extend(RawRecord {
            file_id: 5,
            parent_file_id: 5,
            name: "\\".into(),
            attributes: 0x10,
        });
        b.extend(RawRecord {
            file_id: 9,
            parent_file_id: 5,
            name: "Note.TXT".into(),
            attributes: 0,
        });
        b.finish()
    }

    #[test]
    fn header_is_64_bytes() {
        assert_eq!(HEADER_SIZE, 64);
        assert_eq!(std::mem::size_of::<SnapshotHeader>(), 64);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let snap = sample();
        let header = header_for(&snap);
        let decoded = SnapshotHeader::decode(&header.encode()[..]).unwrap();
        assert_eq!(decoded.record_count, 2);
        assert_eq!(decoded.next_usn, 77);
        assert_eq!(decoded.names_len, snap.names().len() as u64);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = b'X';
        assert!(matches!(
            SnapshotHeader::decode(&buf),
            Err(SnapshotError::BadMagic)
        ));
    }

    #[test]
    fn truncated_header_is_rejected() {
        assert!(matches!(
            SnapshotHeader::decode(&[0u8; 40]),
            Err(SnapshotError::LengthMismatch)
        ));
    }

    #[test]
    fn write_snapshot_produces_expected_size() {
        let snap = sample();
        let path = std::env::temp_dir().join(format!("fsfmt-{}.bin", std::process::id()));
        write_snapshot(&path, &snap).unwrap();
        let len = std::fs::metadata(&path).unwrap().len() as usize;
        assert_eq!(
            len,
            HEADER_SIZE + snap.record_count() * 16 + snap.names().len()
        );
        std::fs::remove_file(&path).ok();
    }
}
