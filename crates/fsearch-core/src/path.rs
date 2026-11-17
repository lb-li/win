use ahash::AHashMap;

use crate::index::snapshot::IndexSnapshot;
use crate::model::ROOT;

const MAX_DEPTH: usize = 64;
const CACHE_CAP: usize = 512;

pub struct PathResolver<'a> {
    snap: &'a IndexSnapshot,
    drive: char,
    cache: AHashMap<u32, String>,
    scratch: Vec<u32>,
}

impl<'a> PathResolver<'a> {
    pub fn new(snap: &'a IndexSnapshot, drive: char) -> Self {
        Self {
            snap,
            drive,
            cache: AHashMap::with_capacity(CACHE_CAP),
            scratch: Vec::with_capacity(MAX_DEPTH),
        }
    }

    pub fn file_name(&self, idx: u32) -> &str {
        let r = &self.snap.records()[idx as usize];
        std::str::from_utf8(self.snap.name_at(r.name_off, r.name_len)).unwrap_or("")
    }

    pub fn depth(&self, idx: u32) -> u16 {
        let mut cur = idx;
        let mut depth = 0u16;
        for _ in 0..MAX_DEPTH {
            let r = &self.snap.records()[cur as usize];
            if r.parent == ROOT {
                return depth;
            }
            depth += 1;
            cur = r.parent;
        }
        depth
    }

    pub fn path_of(&mut self, idx: u32) -> String {
        if let Some(hit) = self.cache.get(&idx) {
            return hit.clone();
        }
        self.scratch.clear();
        let mut cur = idx;
        let mut total = 2usize;
        for _ in 0..MAX_DEPTH {
            let r = &self.snap.records()[cur as usize];
            self.scratch.push(cur);
            total += r.name_len as usize + 1;
            if r.parent == ROOT {
                break;
            }
            cur = r.parent;
        }

        let mut out = String::with_capacity(total);
        out.push(self.drive);
        out.push(':');
        for &node in self.scratch.iter().rev() {
            let r = &self.snap.records()[node as usize];
            let name = self.snap.name_at(r.name_off, r.name_len);
            if name == b"\\" {
                continue;
            }
            out.push('\\');
            out.push_str(std::str::from_utf8(name).unwrap_or(""));
        }
        if out.len() == 2 {
            out.push('\\');
        }

        if self.cache.len() >= CACHE_CAP {
            self.cache.clear();
        }
        self.cache.insert(idx, out.clone());
        out
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
            (11, 10, "Alice", true),
            (12, 11, "notes.txt", false),
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
    fn resolves_full_path_in_lowercase() {
        let s = snap();
        let mut r = PathResolver::new(&s, 'C');
        assert_eq!(r.path_of(3), "C:\\users\\alice\\notes.txt");
    }

    #[test]
    fn depth_counts_segments() {
        let s = snap();
        let r = PathResolver::new(&s, 'C');
        assert_eq!(r.depth(3), 3);
    }

    #[test]
    fn file_name_is_lowercased_slice() {
        let s = snap();
        let r = PathResolver::new(&s, 'C');
        assert_eq!(r.file_name(3), "notes.txt");
    }
}
