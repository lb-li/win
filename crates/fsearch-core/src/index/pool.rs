use crate::model::ascii_lower;

#[derive(Debug, Default, Clone)]
pub struct NamePool {
    bytes: Vec<u8>,
}

impl NamePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
        }
    }

    pub fn push(&mut self, name: &str) -> (u32, u16) {
        let lowered = ascii_lower(name);
        self.push_bytes(lowered.as_bytes())
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> (u32, u16) {
        assert!(bytes.len() <= u16::MAX as usize, "name too long");
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(bytes);
        (off, bytes.len() as u16)
    }

    pub fn get(&self, off: u32, len: u16) -> &[u8] {
        let start = off as usize;
        &self.bytes[start..start + len as usize]
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_returns_offsets_and_lowercases_ascii() {
        let mut pool = NamePool::new();
        let (off_a, len_a) = pool.push("README.TXT");
        let (off_b, len_b) = pool.push("Main.RS");
        assert_eq!((off_a, len_a), (0, 10));
        assert_eq!(off_b, 10);
        assert_eq!(pool.get(off_a, len_a), b"readme.txt");
        assert_eq!(pool.get(off_b, len_b), b"main.rs");
    }

    #[test]
    fn push_keeps_non_ascii_bytes_untouched() {
        let mut pool = NamePool::new();
        let (off, len) = pool.push("报告ABC.pdf");
        assert_eq!(pool.get(off, len), "报告abc.pdf".as_bytes());
        assert_eq!(len as usize, "报告ABC.pdf".len());
    }

    #[test]
    fn push_bytes_does_not_lowercase() {
        let mut pool = NamePool::new();
        let (off, len) = pool.push_bytes(b"MixedCase");
        assert_eq!(pool.get(off, len), b"MixedCase");
    }
}
