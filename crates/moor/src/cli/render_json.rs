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

/// Hand-built minimal failed envelope used when serialisation fails.
fn fallback(cmd: &str) -> String {
    let cmd = serde_json::to_string(cmd).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        "{{\"schema\":\"{}\",\"cmd\":{cmd},\"class\":\"failed\",\"value\":null,\"warnings\":[],\"error\":{{\"kind\":\"internal\",\"message\":\"response could not be serialised\",\"hints\":[],\"context\":[]}}}}\n",
        moor_core::SCHEMA
    )
}
