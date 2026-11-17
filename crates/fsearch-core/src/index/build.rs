use ahash::AHashMap;

use crate::collector::RawRecord;
use crate::index::snapshot::{IndexSnapshot, Store};
use crate::index::NamePool;
use crate::model::{flags_from_attributes, Record, ROOT};

pub struct IndexBuilder {
    volume_id: u8,
    next_usn: i64,
    raw: Vec<RawRecord>,
}

impl IndexBuilder {
    pub fn new(volume_id: u8, next_usn: i64) -> Self {
        Self {
            volume_id,
            next_usn,
            raw: Vec::new(),
        }
    }

    pub fn with_capacity(volume_id: u8, next_usn: i64, cap: usize) -> Self {
        Self {
            volume_id,
            next_usn,
            raw: Vec::with_capacity(cap),
        }
    }

    pub fn extend(&mut self, record: RawRecord) {
        if !record.name.is_empty() {
            self.raw.push(record);
        }
    }

    pub fn finish(self) -> IndexSnapshot {
        let n = self.raw.len();
        let mut records: Vec<Record> = Vec::with_capacity(n);
        let mut pool = NamePool::with_capacity(n * 16);
        let mut by_id: AHashMap<u64, u32> = AHashMap::with_capacity(n);
        let mut seeds = vec![false; n];

        for (i, r) in self.raw.iter().enumerate() {
            by_id.insert(r.file_id, i as u32);
            let (off, len) = pool.push(&r.name);
            records.push(Record {
                name_off: off,
                parent: ROOT,
                file_id: r.file_id as u32,
                flags: flags_from_attributes(r.attributes),
                name_len: len,
            });
            seeds[i] = r.parent_file_id == r.file_id;
        }

        for (i, (record, raw)) in records.iter_mut().zip(self.raw.iter()).enumerate() {
            if let Some(&p) = by_id.get(&raw.parent_file_id) {
                if p != i as u32 {
                    record.parent = p;
                }
            }
        }

        let reachable = Self::reachable(&records, &seeds);
        let mut remap = vec![u32::MAX; n];
        let mut kept: Vec<Record> = Vec::with_capacity(n);
        for (i, r) in records.iter().enumerate() {
            if reachable[i] {
                remap[i] = kept.len() as u32;
                kept.push(*r);
            }
        }
        for r in kept.iter_mut() {
            if r.parent != ROOT {
                r.parent = remap[r.parent as usize];
            }
        }

        let mut order: Vec<u32> = (0..kept.len() as u32).collect();
        order.sort_unstable_by_key(|&i| kept[i as usize].file_id);
        let mut pos = vec![0u32; kept.len()];
        for (new_idx, &old_idx) in order.iter().enumerate() {
            pos[old_idx as usize] = new_idx as u32;
        }
        let mut sorted: Vec<Record> = Vec::with_capacity(kept.len());
        for &old_idx in &order {
            let mut r = kept[old_idx as usize];
            if r.parent != ROOT {
                r.parent = pos[r.parent as usize];
            }
            sorted.push(r);
        }

        let names = Self::rebuild_pool(&mut sorted, &pool);
        IndexSnapshot::new(
            self.volume_id,
            0,
            0,
            self.next_usn,
            0,
            Store::Owned {
                records: sorted,
                names,
            },
        )
    }

    fn reachable(records: &[Record], seeds: &[bool]) -> Vec<bool> {
        let mut children: Vec<Vec<u32>> = vec![Vec::new(); records.len()];
        let mut stack: Vec<u32> = Vec::new();
        for (i, r) in records.iter().enumerate() {
            if r.parent != ROOT {
                children[r.parent as usize].push(i as u32);
            } else if seeds[i] {
                stack.push(i as u32);
            }
        }
        let mut ok = vec![false; records.len()];
        for &s in &stack {
            ok[s as usize] = true;
        }
        while let Some(cur) = stack.pop() {
            for &c in &children[cur as usize] {
                if !ok[c as usize] {
                    ok[c as usize] = true;
                    stack.push(c);
                }
            }
        }
        ok
    }

    fn rebuild_pool(records: &mut [Record], old: &NamePool) -> NamePool {
        let mut fresh = NamePool::with_capacity(old.len());
        for r in records.iter_mut() {
            let bytes = old.get(r.name_off, r.name_len);
            let (off, len) = fresh.push_bytes(bytes);
            r.name_off = off;
            r.name_len = len;
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{F_DIR, ROOT};

    fn raw(file_id: u64, parent: u64, name: &str, dir: bool) -> RawRecord {
        RawRecord {
            file_id,
            parent_file_id: parent,
            name: name.to_string(),
            attributes: if dir { 0x10 } else { 0 },
        }
    }

    fn sample() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 42);
        b.extend(raw(5, 5, "\\", true));
        b.extend(raw(10, 5, "Users", true));
        b.extend(raw(11, 10, "Alice", true));
        b.extend(raw(12, 11, "Notes.TXT", false));
        b.finish()
    }

    #[test]
    fn records_are_sorted_by_file_id() {
        let ids: Vec<u32> = sample().records().iter().map(|r| r.file_id).collect();
        assert_eq!(ids, vec![5, 10, 11, 12]);
    }

    #[test]
    fn parent_is_resolved_to_index() {
        let snap = sample();
        assert_eq!(snap.records()[3].parent, 2);
        assert_eq!(snap.records()[2].parent, 1);
        assert_eq!(snap.records()[1].parent, 0);
        assert_eq!(snap.records()[0].parent, ROOT);
    }

    #[test]
    fn names_are_lowercased_and_pool_is_in_record_order() {
        let snap = sample();
        let notes = &snap.records()[3];
        assert_eq!(snap.name_at(notes.name_off, notes.name_len), b"notes.txt");
        let mut pos = 0usize;
        for r in snap.records() {
            assert_eq!(r.name_off as usize, pos);
            pos += r.name_len as usize;
        }
    }

    #[test]
    fn orphan_records_are_dropped() {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(raw(5, 5, "\\", true));
        b.extend(raw(99, 777, "orphan.txt", false));
        assert_eq!(b.finish().record_count(), 1);
    }

    #[test]
    fn cycles_are_dropped_not_hung() {
        let mut b = IndexBuilder::new(0, 0);
        b.extend(raw(5, 6, "a", true));
        b.extend(raw(6, 5, "b", true));
        assert_eq!(b.finish().record_count(), 0);
    }

    #[test]
    fn directories_get_dir_flag() {
        let snap = sample();
        assert_ne!(snap.records()[1].flags & F_DIR, 0);
        assert_eq!(snap.records()[3].flags & F_DIR, 0);
    }

    #[test]
    fn next_usn_is_carried_over() {
        assert_eq!(sample().next_usn, 42);
    }
}
