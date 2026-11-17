pub const RADIUS: f32 = 12.0;
pub const GAP: f32 = 8.0;
pub const BG: &str = "#0f1115";
pub const SURFACE: &str = "#171a21";
pub const TEXT: &str = "#eef1f6";
pub const MUTED: &str = "#8b929e";
pub const ACCENT: &str = "#4c8dff";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_slint_values() {
        assert_eq!(RADIUS, 12.0);
        assert_eq!(GAP, 8.0);
        assert_eq!(BG, "#0f1115");
        assert_eq!(SURFACE, "#171a21");
        assert_eq!(TEXT, "#eef1f6");
        assert_eq!(MUTED, "#8b929e");
        assert_eq!(ACCENT, "#4c8dff");
    }
}
