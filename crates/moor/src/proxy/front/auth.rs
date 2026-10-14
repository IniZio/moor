//! Proxy credential check and head rewrite for the front listener.

use moor_core::Secret;
use zeroize::Zeroizing;

/// Lines of a head already validated as UTF-8 ending in a blank line.
fn lines(head: &[u8]) -> Option<impl Iterator<Item = &str>> {
    let text = std::str::from_utf8(head).ok()?;
    Some(text.strip_suffix("\r\n\r\n")?.split("\r\n"))
}

/// Value of a `Proxy-Authorization` header line, `None` for any other line.
fn proxy_auth_value(line: &str) -> Option<&str> {
    let (name, value) = line.split_once(':')?;
    name.eq_ignore_ascii_case("proxy-authorization")
        .then(|| value.trim_matches([' ', '\t']))
}

/// Compares in time that depends only on the lengths, not on where the
/// contents differ.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let diff = a
        .iter()
        .zip(b)
        .fold(a.len() ^ b.len(), |acc, (x, y)| acc | usize::from(x ^ y));
    diff == 0
}

/// Value of one base64 alphabet byte.
fn sextet(b: u8) -> Option<u32> {
    match b {
        b'A'..=b'Z' => Some(u32::from(b - b'A')),
        b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Strict standard base64: padded, padding only at the end.
fn decode(text: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() / 4 * 3));
    let quads = text.len() / 4;
    for (i, quad) in text.chunks(4).enumerate() {
        let mut acc = 0_u32;
        let mut pad = 0_usize;
        for &b in quad {
            acc <<= 6;
            if b == b'=' {
                pad += 1;
            } else if pad == 0 {
                acc |= sextet(b)?;
            } else {
                return None;
            }
        }
        if pad > 2 || (pad > 0 && i + 1 != quads) {
            return None;
        }
        out.extend_from_slice(acc.to_be_bytes().get(1..4 - pad)?);
    }
    Some(out)
}

/// True when `value` is `Basic base64(user:nonce)` or `Bearer nonce`.
fn credential_ok(value: &str, nonce: &Secret) -> bool {
    let Some((scheme, rest)) = value.split_once(' ') else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("bearer") {
        return ct_eq(rest.as_bytes(), nonce.expose());
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let Some(decoded) = decode(rest.as_bytes()) else {
            return false;
        };
        let Some(colon) = decoded.iter().position(|b| *b == b':') else {
            return false;
        };
        return decoded
            .get(colon + 1..)
            .is_some_and(|password| ct_eq(password, nonce.expose()));
    }
    false
}

/// Checks the single `Proxy-Authorization` header against `nonce`; on a match
/// returns the head with that header replaced by `Bearer <internal>`.
pub(super) fn authorize(
    head: &[u8],
    nonce: &Secret,
    internal: &Secret,
) -> Option<Zeroizing<Vec<u8>>> {
    let mut values = lines(head)?.filter_map(proxy_auth_value);
    let value = values.next()?;
    if values.next().is_some() || !credential_ok(value, nonce) {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(
        head.len() + internal.expose().len() + 32,
    ));
    for line in lines(head)?.filter(|l| proxy_auth_value(l).is_none()) {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Proxy-Authorization: Bearer ");
    out.extend_from_slice(internal.expose());
    out.extend_from_slice(b"\r\n\r\n");
    Some(out)
}
