//! Text for `close-failed`.

use std::fmt::Write as _;

use serde_json::Value;

/// Text form of `close-failed`: a line per Box closed and per Box that stayed open.
pub(super) fn text(value: &Value) -> Option<String> {
    let closed = value.get("closed")?.as_array()?;
    let failed = value.get("failed")?.as_array()?;
    if closed.is_empty() && failed.is_empty() {
        return Some("no setup_failed Boxes\n".to_owned());
    }
    let mut out = String::new();
    for name in closed.iter().filter_map(Value::as_str) {
        let _ = writeln!(out, "closed {name}");
    }
    for item in failed {
        let name = item.get("box")?.as_str()?;
        let error = item.get("error")?.as_str()?;
        let _ = writeln!(out, "could not close {name}: {error}");
    }
    Some(out)
}
