//! Request body framing and size limits for the loopback proxy.
//!
//! A request whose body length can be read two ways is rejected, so the
//! proxy and the upstream never disagree about where a body ends.

use moor_core::proxy_gate::RouteId;

/// Largest declared body accepted on any route without its own limit, in bytes.
const DEFAULT_BODY_CAP: u64 = 32 * 1024 * 1024;
/// Largest declared body accepted on the git route, in bytes.
const GIT_BODY_CAP: u64 = 2 * 1024 * 1024 * 1024;
/// Route identifier of the git route.
const GIT_ROUTE: &str = "forge.git";

/// How the request body is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyLen {
    /// The request has no body.
    None,
    /// The body is exactly this many bytes.
    Length(u64),
    /// The body is sent in chunks.
    Chunked,
}

/// Request framing that cannot be read one way only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ambiguous;

/// Reads how the request body is delimited from its header fields.
///
/// Header names match case-insensitively. Anything that allows two readings
/// of the body length is [`Ambiguous`].
pub(super) fn body_len(headers: &[(String, String)]) -> Result<BodyLen, Ambiguous> {
    let mut lengths = named(headers, "content-length");
    let length = lengths.next();
    if lengths.next().is_some() {
        return Err(Ambiguous);
    }
    let mut codings = named(headers, "transfer-encoding").peekable();
    let has_te = codings.peek().is_some();
    match (length, has_te) {
        (Some(_), true) => Err(Ambiguous),
        (Some(value), false) => content_length(value).map(BodyLen::Length),
        (None, true) => {
            let joined = codings.collect::<Vec<_>>().join(",");
            chunked_last(&joined).map(|()| BodyLen::Chunked)
        }
        (None, false) => Ok(BodyLen::None),
    }
}

/// Largest declared body accepted on `route`, in bytes.
pub(super) fn body_cap(route: &RouteId) -> u64 {
    if route.as_str() == GIT_ROUTE {
        GIT_BODY_CAP
    } else {
        DEFAULT_BODY_CAP
    }
}

/// Values of the header fields called `name`, in wire order.
fn named<'a>(headers: &'a [(String, String)], name: &'a str) -> impl Iterator<Item = &'a str> {
    headers
        .iter()
        .filter(move |(field, _)| field.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Trims the spaces and tabs that may surround a field value or list element.
fn trim_ows(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

/// Parses a `Content-Length` value, accepting a list whose elements are all the same number.
fn content_length(value: &str) -> Result<u64, Ambiguous> {
    let mut elements = value.split(',').map(trim_ows).map(digits);
    let first = elements.next().ok_or(Ambiguous)??;
    for element in elements {
        if element? != first {
            return Err(Ambiguous);
        }
    }
    Ok(first)
}

/// Parses a non-empty run of ASCII digits that fits in `u64`.
fn digits(text: &str) -> Result<u64, Ambiguous> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Ambiguous);
    }
    text.parse().map_err(|_| Ambiguous)
}

/// Accepts a coding list only when no element is empty and the last is `chunked`.
fn chunked_last(codings: &str) -> Result<(), Ambiguous> {
    let mut last = "";
    for coding in codings.split(',').map(trim_ows) {
        if coding.is_empty() {
            return Err(Ambiguous);
        }
        last = coding;
    }
    if last.eq_ignore_ascii_case("chunked") {
        Ok(())
    } else {
        Err(Ambiguous)
    }
}

#[cfg(test)]
mod tests;
