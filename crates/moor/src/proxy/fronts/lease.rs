//! The lease token a request presents: the password of its `Proxy-Authorization: Basic`
//! header.

use zeroize::Zeroizing;

/// The password of the first `Proxy-Authorization: Basic` header of `head`; none when the head
/// has no such header or it does not decode.
pub(crate) fn token(head: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let text = std::str::from_utf8(head).ok()?;
    let value = text.split("\r\n").find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("proxy-authorization")
            .then_some(value.trim())
    })?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let pair = decode(encoded.trim())?;
    let colon = pair.iter().position(|b| *b == b':')?;
    let password = pair.get(colon + 1..)?;
    (!password.is_empty()).then(|| Zeroizing::new(password.to_vec()))
}

/// Standard base64 with optional padding; none on any other byte.
fn decode(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    let digits = text.trim_end_matches('=').as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(digits.len() * 3 / 4));
    let (mut acc, mut bits) = (0_u32, 0_u32);
    for &c in digits {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::token;

    #[test]
    fn the_basic_password_is_the_token() {
        let head = b"CONNECT api.anthropic.com:443 HTTP/1.1\r\nHost: x\r\n\
            Proxy-Authorization: Basic bGVhc2U6c2VjcmV0LXRva2Vu\r\n\r\n";
        assert_eq!(
            token(head).as_deref().map(Vec::as_slice),
            Some(&b"secret-token"[..])
        );
    }

    #[test]
    fn no_header_another_scheme_or_no_password_is_no_token() {
        for head in [
            &b"CONNECT a:443 HTTP/1.1\r\n\r\n"[..],
            b"CONNECT a:443 HTTP/1.1\r\nProxy-Authorization: Bearer abc\r\n\r\n",
            b"CONNECT a:443 HTTP/1.1\r\nProxy-Authorization: Basic bGVhc2U6\r\n\r\n",
            b"CONNECT a:443 HTTP/1.1\r\nProxy-Authorization: Basic !!\r\n\r\n",
        ] {
            assert!(token(head).is_none(), "{}", String::from_utf8_lossy(head));
        }
    }
}
