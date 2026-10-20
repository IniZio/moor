//! The internal token on a CONNECT head, in place of any proxy credential the Box sent.

use moor_core::Secret;
use zeroize::Zeroizing;

/// Whether `line` is a `Proxy-Authorization` header line.
fn is_proxy_auth(line: &str) -> bool {
    line.split_once(':')
        .is_some_and(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization"))
}

/// `head`, a head already validated as UTF-8 ending in a blank line, with every
/// `Proxy-Authorization` line dropped and `Bearer <internal>` added as the last header.
pub(super) fn with_token(head: &[u8], internal: &Secret) -> Option<Zeroizing<Vec<u8>>> {
    let text = std::str::from_utf8(head).ok()?;
    let lines = text.strip_suffix("\r\n\r\n")?.split("\r\n");
    let mut out = Zeroizing::new(Vec::with_capacity(
        head.len() + internal.expose().len() + 32,
    ));
    for line in lines.filter(|l| !is_proxy_auth(l)) {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Proxy-Authorization: Bearer ");
    out.extend_from_slice(internal.expose());
    out.extend_from_slice(b"\r\n\r\n");
    Some(out)
}
