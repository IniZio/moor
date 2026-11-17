//! The 403 a refused request is answered with.

use super::REFUSAL_BODY;

/// The 403 a refused request is answered with.
pub(crate) fn refusal() -> String {
    format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{REFUSAL_BODY}",
        REFUSAL_BODY.len()
    )
}
