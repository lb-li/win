use std::time::{Duration, Instant};

use ahash::AHashMap;

use crate::index::overlay::Overlay;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePair {
    pub file_id: u32,
    pub old_name: Option<String>,
    pub new_name: Option<String>,
}

pub struct RenameBuffer {
    timeout: Duration,
    pending: AHashMap<u32, (Instant, String)>,
}

impl RenameBuffer {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            pending: AHashMap::new(),
        }
    }

    pub fn on_old(&mut self, file_id: u32, old_name: String) {
        self.on_old_at(file_id, old_name, Instant::now());
    }

    pub fn on_old_at(&mut self, file_id: u32, old_name: String, at: Instant) {
        self.pending.insert(file_id, (at, old_name));
    }

    pub fn on_new(&mut self, file_id: u32, new_name: String) -> Option<RenamePair> {
        let old_name = self.pending.remove(&file_id).map(|(_, name)| name);
        Some(RenamePair {
            file_id,
            old_name,
            new_name: Some(new_name),
        })
    }

    pub fn expire(&mut self, now: Instant) -> Vec<RenamePair> {
        let mut expired = Vec::new();
        self.pending.retain(|&file_id, (at, name)| {
            if now.duration_since(*at) >= self.timeout {
                expired.push(RenamePair {
                    file_id,
                    old_name: Some(name.clone()),
                    new_name: None,
                });
                false
            } else {
                true
            }
        });
        expired
    }
}

pub fn should_merge(overlay: &Overlay, total: usize) -> bool {
    if total == 0 {
        return false;
    }
    (overlay.added_count() + overlay.tombstone_count()) * 100 > total * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_old_and_new_name() {
        let mut buf = RenameBuffer::new(Duration::from_secs(2));
        buf.on_old(42, "old.txt".into());
        let pair = buf.on_new(42, "new.txt".into()).expect("should pair");
        assert_eq!(pair.file_id, 42);
        assert_eq!(pair.old_name.as_deref(), Some("old.txt"));
        assert_eq!(pair.new_name.as_deref(), Some("new.txt"));
    }

    #[test]
    fn new_name_without_old_is_a_creation() {
        let mut buf = RenameBuffer::new(Duration::from_secs(2));
        let pair = buf.on_new(7, "fresh.txt".into()).unwrap();
        assert!(pair.old_name.is_none());
        assert_eq!(pair.new_name.as_deref(), Some("fresh.txt"));
    }

    #[test]
    fn stale_entries_expire() {
        let mut buf = RenameBuffer::new(Duration::from_millis(10));
        let t0 = Instant::now();
        buf.on_old_at(9, "gone.txt".into(), t0);
        let expired = buf.expire(t0 + Duration::from_millis(50));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].file_id, 9);
    }

    #[test]
    fn merge_threshold_is_two_percent() {
        let mut o = Overlay::new(1000);
        for i in 0..10 {
            o.tombstone(i);
        }
        assert!(!should_merge(&o, 1000));
        for i in 10..21 {
            o.tombstone(i);
        }
        assert!(should_merge(&o, 1000));
    }

    #[test]
    fn empty_index_never_triggers_merge() {
        assert!(!should_merge(&Overlay::new(0), 0));
    }
}
