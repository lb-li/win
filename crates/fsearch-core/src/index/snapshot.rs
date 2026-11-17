use std::fs::File;
use std::path::Path;

use memmap2::Mmap;

use crate::index::format::{SnapshotError, SnapshotHeader, HEADER_SIZE};
use crate::index::NamePool;
use crate::model::Record;

pub enum Store {
    Owned {
        records: Vec<Record>,
        names: NamePool,
    },
    Mapped {
        map: Mmap,
        record_count: usize,
        names_off: usize,
    },
}

unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Store {
    fn records(&self) -> &[Record] {
        match self {
            Store::Owned { records, .. } => records,
            Store::Mapped {
                map, record_count, ..
            } => {
                let base = &map[HEADER_SIZE..];
                let ptr = base.as_ptr() as *const Record;
                assert_eq!(
                    ptr as usize % std::mem::align_of::<Record>(),
                    0,
                    "mmap payload is not aligned for Record"
                );
                unsafe { std::slice::from_raw_parts(ptr, *record_count) }
            }
        }
    }

    fn names(&self) -> &[u8] {
        match self {
            Store::Owned { names, .. } => names.as_bytes(),
            Store::Mapped { map, names_off, .. } => &map[*names_off..],
        }
    }
}

pub struct IndexSnapshot {
    pub volume_id: u8,
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
    pub generation: u64,
    store: Store,
}

impl IndexSnapshot {
    pub fn new(
        volume_id: u8,
        volume_serial: u64,
        journal_id: u64,
        next_usn: i64,
        generation: u64,
        store: Store,
    ) -> Self {
        Self {
            volume_id,
            volume_serial,
            journal_id,
            next_usn,
            generation,
            store,
        }
    }

    pub fn open(path: &Path) -> Result<Self, SnapshotError> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        let header = SnapshotHeader::decode(&map)?;
        let records_len = header.record_count as usize * std::mem::size_of::<Record>();
        let names_off = HEADER_SIZE + records_len;
        let expected = names_off + header.names_len as usize;
        if map.len() != expected {
            return Err(SnapshotError::LengthMismatch);
        }
        Ok(Self {
            volume_id: 0,
            volume_serial: header.volume_serial,
            journal_id: header.journal_id,
            next_usn: header.next_usn,
            generation: header.generation,
            store: Store::Mapped {
                map,
                record_count: header.record_count as usize,
                names_off,
            },
        })
    }

    pub fn records(&self) -> &[Record] {
        self.store.records()
    }

    pub fn names(&self) -> &[u8] {
        self.store.names()
    }

    pub fn name_at(&self, off: u32, len: u16) -> &[u8] {
        let start = off as usize;
        &self.names()[start..start + len as usize]
    }

    pub fn record_count(&self) -> usize {
        self.records().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;
    use crate::index::format::write_snapshot;

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(1, 100);
        b.extend(RawRecord {
            file_id: 5,
            parent_file_id: 5,
            name: "\\".into(),
            attributes: 0x10,
        });
        b.extend(RawRecord {
            file_id: 7,
            parent_file_id: 5,
            name: "Report.PDF".into(),
            attributes: 0,
        });
        b.finish()
    }

    #[test]
    fn mapped_snapshot_matches_owned_snapshot() {
        let owned = sample();
        let path = std::env::temp_dir().join(format!("fsmap-{}.bin", std::process::id()));
        write_snapshot(&path, &owned).unwrap();

        let mapped = IndexSnapshot::open(&path).unwrap();
        assert_eq!(mapped.record_count(), owned.record_count());
        assert_eq!(mapped.names(), owned.names());
        for (a, b) in mapped.records().iter().zip(owned.records()) {
            assert_eq!(a, b);
        }
        assert_eq!(mapped.next_usn, 100);
        assert_eq!(mapped.journal_id, owned.journal_id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn truncated_file_is_rejected() {
        let path = std::env::temp_dir().join(format!("fsmap-trunc-{}.bin", std::process::id()));
        std::fs::write(&path, b"FSEARCH\0").unwrap();
        assert!(IndexSnapshot::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
