//! What a request head asks the shared egress Proxy to do: the plain-HTTP rewrite, the CONNECT
//! head sent to a Box's nono, the 403 refusal, and what a Box holds of the machine service.
//!
//! Request bytes carry credentials, so nothing here logs them.

pub(crate) use machine::{hold_or_missing, Held, Hold};
pub(crate) use plain::{connect_head, plain_request};
pub(crate) use refusal::refusal;

mod machine;
mod plain;
mod refusal;

/// Body of the refusal.
const REFUSAL_BODY: &str = r#"{"message":"moor: only CONNECT or http:// requests are allowed"}"#;

/// True for `host:port` with a numeric port and no path, userinfo or spacing.
fn authority_ok(authority: &str) -> bool {
    let plain = authority
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b'@' | b'?' | b'#' | b'\\'));
    plain
        && authority.rsplit_once(':').is_some_and(|(host, port)| {
            !host.is_empty()
                && !port.is_empty()
                && port.len() <= 5
                && port.bytes().all(|b| b.is_ascii_digit())
        })
}

/// True for `name: value` with a token name, no folding and no control bytes.
fn header_ok(line: &str) -> bool {
    let clean = line.bytes().all(|b| (b >= 0x20 && b != 0x7f) || b == b'\t');
    clean
        && line.split_once(':').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        })
}

#[cfg(test)]
mod tests;
