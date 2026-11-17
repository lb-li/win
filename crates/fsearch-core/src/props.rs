use std::collections::VecDeque;
use std::path::Path;
use std::time::UNIX_EPOCH;

use ahash::AHashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAttrs {
    pub size: u64,
    pub modified_secs: u64,
}

pub fn read_attrs(path: &Path) -> Option<FileAttrs> {
    let meta = std::fs::metadata(path).ok()?;
    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(FileAttrs {
        size: meta.len(),
        modified_secs,
    })
}

pub struct AttrCache {
    cap: usize,
    map: AHashMap<u32, FileAttrs>,
    order: VecDeque<u32>,
}

impl AttrCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            map: AHashMap::with_capacity(cap),
            order: VecDeque::with_capacity(cap),
        }
    }

    pub fn insert(&mut self, key: u32, attrs: FileAttrs) {
        if self.map.insert(key, attrs).is_none() {
            self.order.push_back(key);
        }
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn get(&self, key: u32) -> Option<FileAttrs> {
        self.map.get(&key).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_size_of_temp_file() {
        let path = std::env::temp_dir().join(format!("fsprops-{}.txt", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let attrs = read_attrs(&path).expect("metadata");
        assert_eq!(attrs.size, 5);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_yields_none() {
        assert!(read_attrs(Path::new("Z:\\definitely\\missing")).is_none());
    }

    #[test]
    fn cache_returns_hit_without_reload() {
        let mut cache = AttrCache::new(4);
        let attrs = FileAttrs {
            size: 10,
            modified_secs: 1,
        };
        cache.insert(7, attrs);
        assert_eq!(cache.get(7), Some(attrs));
        assert_eq!(cache.get(8), None);
    }

    #[test]
    fn cache_evicts_oldest_when_full() {
        let mut cache = AttrCache::new(2);
        cache.insert(
            1,
            FileAttrs {
                size: 1,
                modified_secs: 1,
            },
        );
        cache.insert(
            2,
            FileAttrs {
                size: 2,
                modified_secs: 2,
            },
        );
        cache.insert(
            3,
            FileAttrs {
                size: 3,
                modified_secs: 3,
            },
        );
        assert_eq!(cache.get(1), None);
        assert_eq!(
            cache.get(3),
            Some(FileAttrs {
                size: 3,
                modified_secs: 3
            })
        );
    }
}
