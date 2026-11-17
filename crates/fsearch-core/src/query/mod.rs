pub mod matcher;
pub mod parse;
pub mod rank;

pub use parse::{compile, parse, Query, Seg, Term, WildcardPattern};
