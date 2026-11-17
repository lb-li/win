pub mod change;
pub mod reasons;
pub mod usn_parse;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRecord {
    pub file_id: u64,
    pub parent_file_id: u64,
    pub name: String,
    pub attributes: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RecordBatch {
    pub records: Vec<RawRecord>,
    pub next_usn: i64,
}

#[derive(Debug, Error)]
pub enum CollectError {
    #[error("volume {0} is not accessible")]
    Inaccessible(char),
    #[error("ioctl failed: {0}")]
    Ioctl(String),
    #[error("need administrator privileges")]
    NotElevated,
}

pub trait Collector {
    fn scan(&self) -> Result<RecordBatch, CollectError>;
}
