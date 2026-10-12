//! JSON envelope rendering: one object, one line.

use moor_core::Response;

/// Renders the response as one JSON object followed by a newline.
pub(crate) fn render(resp: &Response) -> String {
    serde_json::to_string(resp).map_or_else(
        |_| fallback(&resp.cmd),
        |mut s| {
            s.push('\n');
            s
        },
    )
}

/// Minimal failed envelope used when serialisation fails; same key order as [`Response`].
fn fallback(cmd: &str) -> String {
    let envelope = serde_json::json!({
        "schema": moor_core::SCHEMA,
        "cmd": cmd,
        "class": "failed",
        "value": null,
        "warnings": [],
        "error": {
            "kind": "internal",
            "message": "response could not be serialised",
            "hints": [],
            "context": [],
        },
    });
    format!("{envelope}\n")
}

#[cfg(test)]
mod tests;
