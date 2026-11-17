use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use smallvec::SmallVec;

use memchr::memmem;

use crate::index::snapshot::IndexSnapshot;
use crate::model::F_DIR;
use crate::query::matcher::wildcard_match;
use crate::query::parse::{parse, Query, Term};

pub(crate) type Ranges = SmallVec<[(u32, u32); 4]>;

#[derive(Debug, Clone)]
pub struct Hit {
    pub record_idx: u32,
    pub volume_id: u8,
    pub score: u32,
    pub highlight: SmallVec<[(u32, u32); 4]>,
}

#[derive(Debug, Clone)]
struct Scored {
    score: u32,
    record_idx: u32,
    highlight: SmallVec<[(u32, u32); 4]>,
}

impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.record_idx == other.record_idx
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.record_idx.cmp(&self.record_idx))
    }
}

pub fn query(snap: &IndexSnapshot, input: &str, limit: usize) -> Vec<Hit> {
    let q = parse(input);
    if q.terms.is_empty() || limit == 0 {
        return Vec::new();
    }

    let finders = build_finders(&q);
    let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(limit + 1);
    for (idx, r) in snap.records().iter().enumerate() {
        let name = snap.name_at(r.name_off, r.name_len);
        let Some((score, highlight)) = score_record(name, r.flags, &q, &finders) else {
            continue;
        };
        let scored = Scored {
            score,
            record_idx: idx as u32,
            highlight,
        };
        if heap.len() < limit {
            heap.push(Reverse(scored));
        } else if let Some(Reverse(min)) = heap.peek() {
            if scored > *min {
                heap.pop();
                heap.push(Reverse(scored));
            }
        }
    }

    let mut out: Vec<Scored> = heap.into_iter().map(|Reverse(s)| s).collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.into_iter()
        .map(|s| Hit {
            record_idx: s.record_idx,
            volume_id: snap.volume_id,
            score: s.score,
            highlight: s.highlight,
        })
        .collect()
}

fn build_finders<'a>(q: &'a Query) -> Vec<Option<memmem::Finder<'a>>> {
    q.terms
        .iter()
        .map(|t| match t {
            Term::Substr(s) => Some(memmem::Finder::new(s.as_bytes())),
            Term::Wildcard(_) => None,
        })
        .collect()
}

#[inline]
pub(crate) fn score_record(
    name: &[u8],
    flags: u16,
    q: &Query,
    finders: &[Option<memmem::Finder<'_>>],
) -> Option<(u32, Ranges)> {
    let mut total = 0u32;
    let mut highlight: SmallVec<[(u32, u32); 4]> = SmallVec::new();

    for (ti, term) in q.terms.iter().enumerate() {
        let ranges = match term {
            Term::Substr(s) => {
                if name.len() < s.len() {
                    return None;
                }
                let finder = finders[ti].as_ref()?;
                let pos = finder.find(name)?;
                SmallVec::from_slice(&[(pos as u32, s.len() as u32)])
            }
            Term::Wildcard(p) => wildcard_match(name, p)?,
        };
        for (pos, _) in &ranges {
            total += 200u32.saturating_sub((*pos).min(100));
        }
        highlight.extend(ranges);
    }

    if let [Term::Substr(s)] = q.terms.as_slice() {
        if name == s.as_bytes() {
            total += 400;
        } else if name.starts_with(s.as_bytes()) {
            total += 300;
        }
    }

    if flags & F_DIR != 0 {
        total = total.saturating_sub(50);
    }

    Some((total, highlight))
}

pub fn query_parallel(
    pool: &rayon::ThreadPool,
    snap: &IndexSnapshot,
    input: &str,
    limit: usize,
) -> Vec<Hit> {
    use rayon::prelude::*;

    let q = parse(input);
    if q.terms.is_empty() || limit == 0 {
        return Vec::new();
    }

    const CHUNK: usize = 3276;
    let records = snap.records();
    let finders = build_finders(&q);
    let partials: Vec<Vec<Scored>> = pool.install(|| {
        records
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(chunk_idx, chunk)| {
                let base = chunk_idx * CHUNK;
                let mut heap: BinaryHeap<Reverse<Scored>> = BinaryHeap::with_capacity(limit + 1);
                for (i, r) in chunk.iter().enumerate() {
                    let name = snap.name_at(r.name_off, r.name_len);
                    let Some((score, highlight)) = score_record(name, r.flags, &q, &finders) else {
                        continue;
                    };
                    let scored = Scored {
                        score,
                        record_idx: (base + i) as u32,
                        highlight,
                    };
                    if heap.len() < limit {
                        heap.push(Reverse(scored));
                    } else if let Some(Reverse(min)) = heap.peek() {
                        if scored > *min {
                            heap.pop();
                            heap.push(Reverse(scored));
                        }
                    }
                }
                heap.into_iter().map(|Reverse(s)| s).collect::<Vec<_>>()
            })
            .collect()
    });

    let mut merged: Vec<Scored> = partials.into_iter().flatten().collect();
    merged.sort_unstable_by(|a, b| b.cmp(a));
    merged.truncate(limit);
    merged
        .into_iter()
        .map(|s| Hit {
            record_idx: s.record_idx,
            volume_id: snap.volume_id,
            score: s.score,
            highlight: s.highlight,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::RawRecord;
    use crate::index::build::IndexBuilder;

    fn snap() -> IndexSnapshot {
        let mut b = IndexBuilder::new(3, 0);
        for (id, parent, name, dir) in [
            (5u64, 5u64, "\\", true),
            (10, 5, "main.rs", false),
            (11, 5, "notes.txt", false),
            (12, 5, "main", false),
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
    fn exact_name_ranks_above_prefix_and_substring() {
        let hits = query(&snap(), "main", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].record_idx, 3);
        assert_eq!(hits[1].record_idx, 1);
    }

    #[test]
    fn multiple_terms_require_all_to_match() {
        let hits = query(&snap(), "main rs", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 1);
    }

    #[test]
    fn wildcard_query_works() {
        let hits = query(&snap(), "*.txt", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record_idx, 2);
    }

    #[test]
    fn volume_id_is_carried_on_hits() {
        assert_eq!(query(&snap(), "main", 10)[0].volume_id, 3);
    }

    #[test]
    fn limit_caps_results() {
        assert_eq!(query(&snap(), "main", 1).len(), 1);
    }

    #[test]
    fn blank_query_returns_empty() {
        assert!(query(&snap(), "  ", 10).is_empty());
    }
}
