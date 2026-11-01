//! Tunnels to an upstream through an HTTP proxy from the standard proxy env.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub(crate) use ureq::{Proxy, ProxyProtocol};

/// Longest wait for the proxy to accept and to answer the `CONNECT`.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest proxy answer head read.
const MAX_HEAD: usize = 8 << 10;

/// Opens a tunnel to `host:port` through `proxy` with an HTTP `CONNECT`.
pub(crate) fn connect(proxy: &Proxy, host: &str, port: u16) -> io::Result<TcpStream> {
    let authority = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let fail = |reason: String| {
        io::Error::other(format!(
            "proxy {}:{} cannot tunnel to {authority}: {reason}",
            proxy.host(),
            proxy.port()
        ))
    };
    if proxy.protocol() != ProxyProtocol::Http || proxy.username().is_some() {
        return Err(fail(
            "proxies with credentials or a scheme other than http:// are not supported".to_owned(),
        ));
    }
    let mut tcp = dial(proxy).map_err(|e| fail(e.to_string()))?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    write!(
        tcp,
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n"
    )
    .map_err(|e| fail(e.to_string()))?;
    let head = read_head(&mut tcp).map_err(|e| fail(e.to_string()))?;
    let status = head.lines().next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if !status.starts_with("HTTP/1.") || code != "200" {
        return Err(fail(format!("proxy answered `{status}`")));
    }
    tcp.set_read_timeout(None)?;
    tcp.set_write_timeout(None)?;
    Ok(tcp)
}

/// Connects to the first proxy address that answers.
fn dial(proxy: &Proxy) -> io::Result<TcpStream> {
    let mut last = io::Error::other("proxy host did not resolve");
    for addr in (proxy.host(), proxy.port()).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, TIMEOUT) {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Reads the answer head byte by byte so no tunnelled byte is consumed.
fn read_head(tcp: &mut TcpStream) -> io::Result<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return Err(io::Error::other("answer head too long"));
        }
        if tcp.read(&mut byte)? == 0 {
            return Err(io::Error::other("proxy closed the connection"));
        }
        head.extend_from_slice(&byte);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
