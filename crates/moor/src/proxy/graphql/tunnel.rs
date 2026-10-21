//! Raw tunnels for destinations other than the judged host: the authority
//! is parsed strictly and bytes are spliced to an address the guard vetted.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Longest wait for one upstream connection attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a tunnel may carry no bytes in either direction.
pub(super) const IDLE_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_mins(10)
};
/// Read and write timeout on each socket; idleness is checked on expiry.
const POLL: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(30)
};
/// Timed-out reads, over both directions, that make a tunnel idle.
const MAX_QUIET_POLLS: u64 = 2 * IDLE_TIMEOUT.as_secs() / POLL.as_secs();
/// Size of one copy.
const CHUNK: usize = 1 << 14;

/// Splits a `CONNECT` authority into host (without IPv6 brackets) and port.
/// Rejects anything but `host:port` with a numeric non-zero port.
pub(super) fn parse_authority(authority: &str) -> Option<(String, u16)> {
    if authority
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '@' | '\\' | '?' | '#'))
    {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        if !host.contains(':') {
            return None;
        }
        (host, tail.strip_prefix(':')?)
    } else {
        authority.rsplit_once(':')?
    };
    if host.is_empty() || (!authority.starts_with('[') && host.contains([':', '[', ']'])) {
        return None;
    }
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match port.parse::<u16>() {
        Ok(p) if p != 0 => Some((host.to_owned(), p)),
        _ => None,
    }
}

/// Dials the vetted addresses in order; the first that answers wins. The
/// destination is only ever dialled at an address the guard returned.
pub(super) fn dial(addrs: &[SocketAddr]) -> Option<TcpStream> {
    addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
}

/// Copies bytes both ways until either side closes, errors or goes idle.
/// `pending` is whatever the client sent after the `CONNECT` head. Bytes
/// are never inspected or logged.
pub(super) fn splice(client: &TcpStream, upstream: &TcpStream, pending: &[u8]) {
    let setup = [client, upstream].iter().all(|s| {
        s.set_read_timeout(Some(POLL)).is_ok()
            && s.set_write_timeout(Some(POLL)).is_ok()
            && s.set_nodelay(true).is_ok()
    });
    if !setup {
        return;
    }
    if !pending.is_empty() && (&*upstream).write_all(pending).is_err() {
        return;
    }
    let quiet = AtomicU64::new(0);
    std::thread::scope(|scope| {
        let up = scope.spawn(|| pump(client, upstream, &quiet));
        pump(upstream, client, &quiet);
        drop(up.join());
    });
    drop(client.shutdown(Shutdown::Both));
    drop(upstream.shutdown(Shutdown::Both));
}

/// Moves bytes from `from` to `to`; on end half-closes `to`, on failure
/// closes both so the opposite pump stops too.
fn pump(from: &TcpStream, to: &TcpStream, quiet: &AtomicU64) {
    let mut buf = [0u8; CHUNK];
    let (mut r, mut w) = (from, to);
    loop {
        match r.read(&mut buf) {
            Ok(0) => {
                drop(to.shutdown(Shutdown::Write));
                return;
            }
            Ok(n) => {
                let Some(chunk) = buf.get(..n) else { break };
                if w.write_all(chunk).is_err() {
                    break;
                }
                quiet.store(0, Ordering::Relaxed);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                let idle = quiet.fetch_add(1, Ordering::Relaxed);
                if idle >= MAX_QUIET_POLLS {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    drop(from.shutdown(Shutdown::Both));
    drop(to.shutdown(Shutdown::Both));
}

#[cfg(test)]
mod tests {
    use super::parse_authority;

    #[test]
    fn accepts_plain_and_bracketed() {
        assert_eq!(parse_authority("a.test:443"), Some(("a.test".into(), 443)));
        assert_eq!(parse_authority("[::1]:8080"), Some(("::1".into(), 8080)));
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "a.test",
            "a.test:",
            ":443",
            "a.test:0",
            "a.test:99999",
            "a.test:+1",
            "a b:443",
            "u@a.test:443",
            "a.test/x:443",
            "::1:443",
            "[a.test]:443",
            "[::1]443",
            "a:b:443",
        ] {
            assert_eq!(parse_authority(bad), None, "{bad}");
        }
    }
}
