//! What a forwarder's request head asks for.

use zeroize::Zeroizing;

use crate::proxy::egress::{PLAIN, TARGET};
use crate::proxy::fronts::lease::basic_password;

/// What the forwarder asked for.
pub(super) struct Ask {
    /// The request path.
    pub(super) path: String,
    /// `Sec-WebSocket-Key`, when it asked to upgrade.
    pub(super) key: Option<String>,
    /// The bearer token.
    pub(super) token: Option<Zeroizing<Vec<u8>>>,
    /// `X-Moor-Target`, when it is `host:port`.
    pub(super) target: Option<String>,
    /// Whether `X-Moor-Plain` marks one plain `http://` request.
    pub(super) plain: bool,
}

/// The forwarder's request, or `None` when it is not one HTTP/1.1 request head.
pub(super) fn parse(head: &[u8]) -> Option<Ask> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    if !matches!(req.parse(head), Ok(httparse::Status::Complete(_))) || req.method != Some("GET") {
        return None;
    }
    let find = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value)
    };
    let text = |name: &str| find(name).and_then(|v| std::str::from_utf8(v).ok());
    let upgrade = text("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let token = find("authorization").and_then(|v| {
        v.strip_prefix(b"Bearer ")
            .filter(|t| !t.is_empty())
            .map(|t| Zeroizing::new(t.to_vec()))
            .or_else(|| std::str::from_utf8(v).ok().and_then(basic_password))
    });
    Some(Ask {
        path: req.path?.to_owned(),
        key: text("sec-websocket-key")
            .filter(|_| upgrade)
            .map(str::to_owned),
        token,
        target: text(TARGET).filter(|t| authority_ok(t)).map(str::to_owned),
        plain: text(PLAIN) == Some("1"),
    })
}

/// True for `host:port` or `[v6]:port` with a port that fits and no path, userinfo or spacing.
pub(in crate::proxy::egress) fn authority_ok(authority: &str) -> bool {
    let plain = authority
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'/' | b'@' | b'?' | b'#' | b'\\'));
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    let host_ok = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .map_or_else(
            || !host.is_empty() && !host.contains([':', '[', ']']),
            |v6| v6.parse::<std::net::Ipv6Addr>().is_ok(),
        );
    plain && host_ok && port.parse::<u16>().is_ok()
}
