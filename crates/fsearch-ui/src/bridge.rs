use std::path::Path;

use fsearch_core::index::snapshot::IndexSnapshot;
use fsearch_core::path::PathResolver;
use fsearch_core::props::{read_attrs, AttrCache};
use fsearch_core::query::rank::Hit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub before: String,
    pub matched: String,
    pub after: String,
    pub path: String,
    pub size: String,
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn row_from_hit(name: &[u8], path: &str, size: u64, hit: &Hit) -> Row {
    let name = String::from_utf8_lossy(name).into_owned();
    let shown_size = format_size(size);
    match hit.highlight.first() {
        Some(&(start, len)) => {
            let mut s = (start as usize).min(name.len());
            let mut e = (s + len as usize).min(name.len());
            while s > 0 && !name.is_char_boundary(s) {
                s -= 1;
            }
            while e < name.len() && !name.is_char_boundary(e) {
                e += 1;
            }
            Row {
                before: name[..s].to_string(),
                matched: name[s..e].to_string(),
                after: name[e..].to_string(),
                path: path.to_string(),
                size: shown_size,
            }
        }
        None => Row {
            before: name,
            matched: String::new(),
            after: String::new(),
            path: path.to_string(),
            size: shown_size,
        },
    }
}

pub fn rows_from_hits(snap: &IndexSnapshot, drive: char, hits: &[Hit]) -> Vec<Row> {
    let mut resolver = PathResolver::new(snap, drive);
    let mut cache = AttrCache::new(256);
    hits.iter()
        .map(|hit| {
            let record = &snap.records()[hit.record_idx as usize];
            let name = snap.name_at(record.name_off, record.name_len);
            let path = resolver.path_of(hit.record_idx);
            let size = match cache.get(hit.record_idx) {
                Some(attrs) => attrs.size,
                None => match read_attrs(Path::new(&path)) {
                    Some(attrs) => {
                        cache.insert(hit.record_idx, attrs);
                        attrs.size
                    }
                    None => 0,
                },
            };
            row_from_hit(name, &path, size, hit)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    fn hit(highlight: &[(u32, u32)]) -> Hit {
        Hit {
            record_idx: 0,
            volume_id: 0,
            score: 1,
            highlight: SmallVec::from_slice(highlight),
        }
    }

    #[test]
    fn splits_name_around_first_highlight() {
        let row = row_from_hit(b"notes.txt", "C:\\notes.txt", 2048, &hit(&[(2, 3)]));
        assert_eq!(row.before, "no");
        assert_eq!(row.matched, "tes");
        assert_eq!(row.after, ".txt");
        assert_eq!(row.path, "C:\\notes.txt");
        assert_eq!(row.size, "2.0 KB");
    }

    #[test]
    fn no_highlight_puts_everything_in_before() {
        let row = row_from_hit(b"notes.txt", "C:\\notes.txt", 10, &hit(&[]));
        assert_eq!(row.before, "notes.txt");
        assert_eq!(row.matched, "");
        assert_eq!(row.after, "");
        assert_eq!(row.size, "10 B");
    }

    #[test]
    fn invalid_utf8_is_lossy_not_panic() {
        let row = row_from_hit(&[0xff, 0xfe], "x", 0, &hit(&[]));
        assert!(!row.before.is_empty());
    }

    #[test]
    fn format_size_scales_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
