//! The agent contract this build runs, raw and whole, for the portal's contract page.

use moor_core::contract::{contract, sha256, SOURCE};
use serde_json::{json, Value};

/// Where the page reads it.
pub(super) const PATH: &str = "/account/api/contract";

/// The running contract: its version, the sha256 of its file and the file itself.
pub(super) fn view() -> Value {
    match contract() {
        Ok(c) => json!({"version": c.version, "sha256": sha256(SOURCE), "text": SOURCE}),
        Err(why) => json!({"error": why}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_gets_the_raw_file_with_its_version() {
        let v = view();
        let field = |k: &str| v.get(k).cloned().unwrap_or_default();
        assert_eq!(field("text"), SOURCE);
        assert_eq!(field("version"), contract().unwrap().version);
        assert_eq!(field("sha256").as_str().map(str::len), Some(64));
    }
}
