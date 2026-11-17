use crate::model::ascii_lower;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Term {
    Substr(String),
    Wildcard(WildcardPattern),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WildcardPattern {
    pub segs: Vec<Seg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Literal(String),
    One,
    Many,
}

pub fn parse(input: &str) -> Query {
    let terms = split_terms(input)
        .into_iter()
        .filter(|t| !t.is_empty())
        .map(|t| compile(&t))
        .collect();
    Query { terms }
}

pub fn split_terms(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn compile(term: &str) -> Term {
    if !term.contains('*') && !term.contains('?') {
        return Term::Substr(ascii_lower(term));
    }
    let mut segs = Vec::new();
    let mut lit = String::new();
    for c in term.chars() {
        match c {
            '*' => {
                if !lit.is_empty() {
                    segs.push(Seg::Literal(ascii_lower(&lit)));
                    lit.clear();
                }
                if !matches!(segs.last(), Some(Seg::Many)) {
                    segs.push(Seg::Many);
                }
            }
            '?' => {
                if !lit.is_empty() {
                    segs.push(Seg::Literal(ascii_lower(&lit)));
                    lit.clear();
                }
                segs.push(Seg::One);
            }
            _ => lit.push(c),
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Literal(ascii_lower(&lit)));
    }
    Term::Wildcard(WildcardPattern { segs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_spaces() {
        let q = parse("read me");
        assert_eq!(q.terms.len(), 2);
        assert_eq!(q.terms[0], Term::Substr("read".into()));
        assert_eq!(q.terms[1], Term::Substr("me".into()));
    }

    #[test]
    fn quoted_phrase_keeps_spaces_and_lowercases() {
        let q = parse("\"My File\" txt");
        assert_eq!(q.terms[0], Term::Substr("my file".into()));
        assert_eq!(q.terms[1], Term::Substr("txt".into()));
    }

    #[test]
    fn star_and_question_compile_to_segments() {
        let q = parse("*.RS");
        let Term::Wildcard(p) = &q.terms[0] else {
            panic!("expected wildcard")
        };
        assert_eq!(p.segs, vec![Seg::Many, Seg::Literal(".rs".into())]);
    }

    #[test]
    fn consecutive_stars_collapse() {
        let q = parse("a**b");
        let Term::Wildcard(p) = &q.terms[0] else {
            panic!()
        };
        assert_eq!(
            p.segs,
            vec![
                Seg::Literal("a".into()),
                Seg::Many,
                Seg::Literal("b".into())
            ]
        );
    }

    #[test]
    fn blank_input_yields_no_terms() {
        assert!(parse("   ").terms.is_empty());
    }
}
