//! Compile-time checks on the wrapper script.

/// Whether `s` holds a `'`.
#[expect(clippy::indexing_slicing, reason = "i stays below the length")]
pub(super) const fn has_single_quote(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            return true;
        }
        i += 1;
    }
    false
}
