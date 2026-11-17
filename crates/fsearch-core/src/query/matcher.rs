use memchr::memmem;
use smallvec::SmallVec;

use crate::query::parse::{Seg, Term, WildcardPattern};

pub fn contains(hay: &[u8], needle: &[u8]) -> Option<(u32, u32)> {
    if needle.is_empty() {
        return None;
    }
    memmem::find(hay, needle).map(|pos| (pos as u32, needle.len() as u32))
}

pub fn wildcard_match(hay: &[u8], pat: &WildcardPattern) -> Option<SmallVec<[(u32, u32); 4]>> {
    let segs = &pat.segs;
    if segs.is_empty() {
        return if hay.is_empty() {
            Some(SmallVec::new())
        } else {
            None
        };
    }

    let anchored_end = !matches!(segs.last(), Some(Seg::Many));
    let mut out: SmallVec<[(u32, u32); 4]> = SmallVec::new();
    let mut hi = 0usize;

    for (si, seg) in segs.iter().enumerate() {
        match seg {
            Seg::Many => {}
            Seg::One => {
                if hi >= hay.len() {
                    return None;
                }
                hi += 1;
            }
            Seg::Literal(lit) => {
                let lb = lit.as_bytes();
                let after_many = si > 0 && matches!(segs[si - 1], Seg::Many);
                let pos = if after_many {
                    memmem::find(&hay[hi..], lb)? + hi
                } else if hay[hi..].starts_with(lb) {
                    hi
                } else {
                    return None;
                };
                out.push((pos as u32, lb.len() as u32));
                hi = pos + lb.len();
            }
        }
    }

    if anchored_end && hi != hay.len() {
        return None;
    }
    Some(out)
}

pub fn match_term(name: &[u8], term: &Term) -> Option<SmallVec<[(u32, u32); 4]>> {
    match term {
        Term::Substr(s) => contains(name, s.as_bytes()).map(|r| SmallVec::from_slice(&[r])),
        Term::Wildcard(p) => wildcard_match(name, p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(segs: Vec<Seg>) -> WildcardPattern {
        WildcardPattern { segs }
    }

    #[test]
    fn substring_reports_position() {
        assert_eq!(contains(b"main.rs", b"ain"), Some((1, 3)));
        assert_eq!(contains(b"main.rs", b"zzz"), None);
    }

    #[test]
    fn suffix_wildcard_must_anchor_at_end() {
        let p = pat(vec![Seg::Many, Seg::Literal(".rs".into())]);
        assert!(wildcard_match(b"main.rs", &p).is_some());
        assert!(wildcard_match(b"main.rs.txt", &p).is_none());
    }

    #[test]
    fn prefix_wildcard_must_anchor_at_start() {
        let p = pat(vec![Seg::Literal("read".into()), Seg::Many]);
        assert!(wildcard_match(b"readme.txt", &p).is_some());
        assert!(wildcard_match(b"myreadme", &p).is_none());
    }

    #[test]
    fn question_mark_consumes_exactly_one_byte() {
        let p = pat(vec![
            Seg::Literal("a".into()),
            Seg::One,
            Seg::Literal("c".into()),
        ]);
        assert!(wildcard_match(b"abc", &p).is_some());
        assert!(wildcard_match(b"ac", &p).is_none());
        assert!(wildcard_match(b"abbc", &p).is_none());
    }

    #[test]
    fn literal_after_one_does_not_search_ahead() {
        let p = pat(vec![
            Seg::Literal("a".into()),
            Seg::One,
            Seg::Literal("b".into()),
        ]);
        assert!(wildcard_match(b"axb", &p).is_some());
        assert!(wildcard_match(b"axxb", &p).is_none());
    }

    #[test]
    fn trailing_one_allows_any_single_char() {
        let p = pat(vec![Seg::Literal("report".into()), Seg::One]);
        assert!(wildcard_match(b"reporta", &p).is_some());
        assert!(wildcard_match(b"report", &p).is_none());
        assert!(wildcard_match(b"reportab", &p).is_none());
    }

    #[test]
    fn middle_literal_reports_highlight_offset() {
        let p = pat(vec![Seg::Many, Seg::Literal("test".into()), Seg::Many]);
        let ranges = wildcard_match(b"mytest.rs", &p).expect("should match");
        assert_eq!(ranges.as_slice(), &[(2, 4)]);
    }

    #[test]
    fn substring_term_highlight_matches_needle() {
        let ranges = match_term(b"notes.txt", &Term::Substr("tes".into())).unwrap();
        assert_eq!(ranges.as_slice(), &[(2, 3)]);
    }
}
