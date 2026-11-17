use crate::index::snapshot::IndexSnapshot;
use crate::index::NamePool;
use crate::model::Record;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate {
    Main(u32),
    Added(u32),
}

pub struct Overlay {
    base_len: u32,
    added: Vec<Record>,
    added_names: NamePool,
    tombstones: Vec<u32>,
}

impl Overlay {
    pub fn new(base_len: usize) -> Self {
        Self {
            base_len: base_len as u32,
            added: Vec::new(),
            added_names: NamePool::new(),
            tombstones: Vec::new(),
        }
    }

    pub fn locate(&self, snap: &IndexSnapshot, file_id: u32) -> Option<Locate> {
        let recs = snap.records();
        let pos = recs.partition_point(|r| r.file_id < file_id);
        if pos < recs.len() && recs[pos].file_id == file_id {
            return Some(Locate::Main(pos as u32));
        }
        self.added
            .iter()
            .position(|r| r.file_id == file_id)
            .map(|i| Locate::Added(i as u32))
    }

    pub fn add(&mut self, file_id: u32, parent: u32, name: &str, flags: u16) -> u32 {
        let (off, len) = self.added_names.push(name);
        let idx = self.added.len() as u32;
        self.added.push(Record {
            name_off: off,
            parent,
            file_id,
            flags,
            name_len: len,
        });
        self.base_len + idx
    }

    pub fn name_at(&self, overlay_idx: u32) -> &[u8] {
        let r = &self.added[overlay_idx as usize];
        self.added_names.get(r.name_off, r.name_len)
    }

    pub fn tombstone(&mut self, idx: u32) {
        let pos = self.tombstones.partition_point(|&t| t < idx);
        if self.tombstones.get(pos) != Some(&idx) {
            self.tombstones.insert(pos, idx);
        }
    }

    pub fn is_tombstoned(&self, idx: u32) -> bool {
        self.tombstones.binary_search(&idx).is_ok()
    }

    pub fn cascade_tombstone(&mut self, snap: &IndexSnapshot, dir_idx: u32) {
        let mut frontier = vec![dir_idx];
        self.tombstone(dir_idx);
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for (i, r) in snap.records().iter().enumerate() {
                let idx = i as u32;
                if self.is_tombstoned(idx) {
                    continue;
                }
                if frontier.contains(&r.parent) {
                    self.tombstone(idx);
                    next.push(idx);
                }
            }
            frontier = next;
        }
    }

    pub fn added_count(&self) -> usize {
        self.added.len()
    }

    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    pub fn added_records(&self) -> &[Record] {
        &self.added
    }

    pub fn tombstones(&self) -> &[u32] {
        &self.tombstones
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn snap() -> IndexSnapshot {
        let mut b = IndexBuilder::new(0, 0);
        for (id, parent, name, dir) in [
            (5u64, 5u64, "\\", true),
            (10, 5, "Users", true),
            (11, 10, "a.txt", false),
            (12, 10, "b.txt", false),
        ] {
            b.extend(RawRecord {
                file_id: id,
                parent_file_id: parent,
                name: name.to_string(),
                attributes: if dir { 0x10 } else { 0 },
            });
        }
        b.finish()
    }

    #[test]
    fn locates_records_in_main_snapshot() {
        let s = snap();
        let o = Overlay::new(s.record_count());
        assert!(matches!(o.locate(&s, 11), Some(Locate::Main(2))));
        assert!(o.locate(&s, 999).is_none());
    }

    #[test]
    fn tombstone_lookup_is_binary_and_order_independent() {
        let mut o = Overlay::new(10);
        o.tombstone(5);
        o.tombstone(1);
        o.tombstone(3);
        assert!(o.is_tombstoned(1));
        assert!(o.is_tombstoned(5));
        assert!(!o.is_tombstoned(2));
        assert_eq!(o.tombstone_count(), 3);
    }

    #[test]
    fn added_records_are_locatable() {
        let s = snap();
        let mut o = Overlay::new(s.record_count());
        let idx = o.add(77, 1, "New.TXT", 0);
        assert_eq!(idx, s.record_count() as u32);
        assert!(matches!(o.locate(&s, 77), Some(Locate::Added(0))));
        assert_eq!(o.name_at(0), b"new.txt");
    }

    #[test]
    fn cascade_marks_whole_subtree() {
        let s = snap();
        let mut o = Overlay::new(s.record_count());
        o.cascade_tombstone(&s, 1);
        assert!(o.is_tombstoned(1));
        assert!(o.is_tombstoned(2));
        assert!(o.is_tombstoned(3));
        assert!(!o.is_tombstoned(0));
    }
}
