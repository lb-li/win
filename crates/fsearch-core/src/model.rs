pub const ROOT: u32 = u32::MAX;

pub const F_DIR: u16 = 1 << 0;
pub const F_HIDDEN: u16 = 1 << 1;
pub const F_SYSTEM: u16 = 1 << 2;
pub const F_READONLY: u16 = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub name_off: u32,
    pub parent: u32,
    pub file_id: u32,
    pub flags: u16,
    pub name_len: u16,
}

pub fn ascii_lower_into(input: &str, out: &mut String) {
    out.clear();
    out.reserve(input.len());
    for c in input.chars() {
        out.push(if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c
        });
    }
}

pub fn ascii_lower(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    ascii_lower_into(input, &mut out);
    out
}

pub fn flags_from_attributes(attributes: u32) -> u16 {
    let mut flags = 0u16;
    if attributes & 0x10 != 0 {
        flags |= F_DIR;
    }
    if attributes & 0x02 != 0 {
        flags |= F_HIDDEN;
    }
    if attributes & 0x04 != 0 {
        flags |= F_SYSTEM;
    }
    if attributes & 0x01 != 0 {
        flags |= F_READONLY;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_is_exactly_16_bytes() {
        assert_eq!(std::mem::size_of::<Record>(), 16);
        assert_eq!(std::mem::align_of::<Record>(), 4);
    }

    #[test]
    fn ascii_lower_preserves_byte_length() {
        for s in ["README.TXT", "İstanbul", "中文ABC", "emoji😀ABC"] {
            assert_eq!(ascii_lower(s).len(), s.len(), "length changed for {s}");
        }
    }

    #[test]
    fn ascii_lower_only_touches_ascii() {
        assert_eq!(ascii_lower("ABC中文"), "abc中文");
    }

    #[test]
    fn attributes_map_to_flags() {
        assert_eq!(flags_from_attributes(0x10), F_DIR);
        assert_eq!(flags_from_attributes(0x02), F_HIDDEN);
        assert_eq!(flags_from_attributes(0x04), F_SYSTEM);
        assert_eq!(flags_from_attributes(0x01), F_READONLY);
        assert_eq!(flags_from_attributes(0x10 | 0x02), F_DIR | F_HIDDEN);
    }
}
