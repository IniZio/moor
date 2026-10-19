//! Plain-HTTP absolute-form requests, turned into a CONNECT toward nono.
//!
//! nono never sees the absolute-form request. The front sends it
//! `CONNECT host:port`, and only after nono accepts writes the origin-form
//! request into the tunnel. The destination guard therefore applies as it does
//! for any CONNECT, and a plain request never carries a credential.

use super::authority_ok;
use super::header_ok;

/// Port assumed when the request names none.
const DEFAULT_PORT: &str = "80";
/// Headers that describe the hop to the proxy, not the request.
const HOP_BY_HOP: [&str; 5] = ["connection", "keep-alive", "te", "trailer", "upgrade"];

/// Headers `Connection` may never remove: they frame or address the request.
const FRAMING: [&str; 3] = ["host", "content-length", "transfer-encoding"];

/// A request rewritten for the tunnel.
pub(super) struct Plain {
    /// `host:port` to ask nono to connect to.
    pub(super) authority: String,
    /// Origin-form request line and cleaned headers, ending in a blank line.
    pub(super) request: Vec<u8>,
}

/// Parses `METHOD http://host[:port]/path HTTP/1.x`; `None` for anything else,
/// including `https://` targets, origin-form targets and malformed heads.
pub(super) fn parse(head: &[u8]) -> Option<Plain> {
    let text = std::str::from_utf8(head).ok()?.strip_suffix("\r\n\r\n")?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return None;
    }
    if method == "CONNECT" || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let (authority, path) = split_target(target)?;
    let headers: Vec<&str> = lines.collect();
    if !headers.iter().all(|line| header_ok(line)) {
        return None;
    }
    let mut request = format!("{method} {path} {version}\r\n").into_bytes();
    request.extend_from_slice(&clean_headers(&headers, &authority));
    Some(Plain { authority, request })
}

/// Splits `http://host[:port]/path` into `host:port` and an origin-form path.
fn split_target(target: &str) -> Option<(String, String)> {
    let scheme = target
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))?;
    let rest = target.get(scheme.len()..)?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(end);
    let authority = match host.rsplit_once(':') {
        Some((_, port)) if !port.contains(']') => host.to_owned(),
        _ => format!("{host}:{DEFAULT_PORT}"),
    };
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let clean = |b: u8| b.is_ascii_graphic() && b != b'#' && b != b'\\';
    (authority_ok(&authority) && path.bytes().all(clean)).then_some((authority, path))
}

/// Name of a header line, lowercased.
fn name_of(line: &str) -> String {
    line.split_once(':')
        .map(|(name, _)| name.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Headers without hop-by-hop fields, `Proxy-*` and anything `Connection`
/// names, plus `Connection: close` and a `Host` when the client sent none.
fn clean_headers(lines: &[&str], authority: &str) -> Vec<u8> {
    let named: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            line.split_once(':')
                .filter(|(n, _)| n.eq_ignore_ascii_case("connection"))
        })
        .flat_map(|(_, value)| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    let mut has_host = false;
    for line in lines {
        let name = name_of(line);
        let framing = FRAMING.contains(&name.as_str());
        let drop = name.starts_with("proxy-")
            || HOP_BY_HOP.contains(&name.as_str())
            || (named.contains(&name) && !framing);
        if !drop {
            has_host |= name == "host";
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }
    if !has_host {
        out.extend_from_slice(format!("Host: {authority}\r\n").as_bytes());
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    out
}

/// The CONNECT head sent to nono for `authority`, carrying `token`.
pub(super) fn connect_head(authority: &str, token: &[u8]) -> zeroize::Zeroizing<Vec<u8>> {
    let mut out = zeroize::Zeroizing::new(Vec::new());
    out.extend_from_slice(
        format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n").as_bytes(),
    );
    out.extend_from_slice(b"Proxy-Authorization: Bearer ");
    out.extend_from_slice(token);
    out.extend_from_slice(b"\r\n\r\n");
    out
}
