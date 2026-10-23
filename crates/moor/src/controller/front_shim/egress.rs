//! The front's egress proxy, `moor front-shim --egress PID`: a helper the outer shim starts
//! while it is still in the host's network. The front has no route out; the outer shim
//! accepts each connection to the front's proxy port and passes it here over the socket on
//! this helper's stdin. Each must ask
//! `CONNECT host:port` for port 443 or 80. The helper resolves the host once, refuses the
//! tunnel when any address it resolved to is denied (see the `denied` module), and connects to
//! the addresses it checked, never resolving again, so a second DNS answer cannot steer it.
//! It holds at most [`MAX_TUNNELS`] tunnels at once and logs one line per decision.

use std::ffi::OsStr;
use std::io::{self, IoSliceMut, Read as _, Write as _};
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs as _};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rustix::net::{
    recvmsg, sendmsg, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};
use rustix::process::{getppid, set_parent_process_death_signal, Pid, Signal};

use super::denied::Denied;

/// The ports a front may open a tunnel to.
const ALLOWED_PORTS: [u16; 2] = [443, 80];

/// The most tunnels one front holds at once; one more is answered 429.
pub(super) const MAX_TUNNELS: usize = 64;

/// The most bytes a CONNECT request may take before its blank line.
const MAX_HEAD: usize = 8 * 1024;

/// How long a connect upstream, and a front's request, may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Passes `conn` to the helper on `helper`.
pub(super) fn pass(helper: &UnixStream, conn: &TcpStream) -> io::Result<()> {
    let fd: BorrowedFd<'_> = std::os::fd::AsFd::as_fd(conn);
    let fds = [fd];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    control.push(SendAncillaryMessage::ScmRights(&fds));
    sendmsg(
        helper,
        &[io::IoSlice::new(b"c")],
        &mut control,
        SendFlags::empty(),
    )?;
    Ok(())
}

/// The next connection the outer shim passed on `shim`; none once the shim is gone. A
/// message without a descriptor is skipped.
fn next(shim: BorrowedFd<'_>) -> Option<TcpStream> {
    loop {
        let mut byte = [0_u8; 1];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let got = recvmsg(
            shim,
            &mut [IoSliceMut::new(&mut byte)],
            &mut control,
            RecvFlags::CMSG_CLOEXEC,
        )
        .ok()?;
        if got.bytes == 0 {
            return None;
        }
        let fd: Option<OwnedFd> = control.drain().find_map(|message| match message {
            RecvAncillaryMessage::ScmRights(mut fds) => fds.next(),
            _ => None,
        });
        if let Some(fd) = fd {
            return Some(TcpStream::from(fd));
        }
    }
}

/// The helper: dies with `target`, its parent, reads the host's addresses and serves each
/// connection the outer shim passes on stdin until the shim is gone.
pub(super) fn helper(target: &OsStr) -> Result<u8, String> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    let target = target
        .to_str()
        .and_then(|t| t.parse().ok())
        .and_then(Pid::from_raw)
        .ok_or("--egress needs a pid")?;
    if getppid() != Some(target) {
        return Err("the outer shim is gone".to_owned());
    }
    let denied = Arc::new(Denied::new(host_addresses()?));
    let open = Arc::new(AtomicUsize::new(0));
    while let Some(conn) = next(rustix::stdio::stdin()) {
        let (denied, open) = (Arc::clone(&denied), Arc::clone(&open));
        let _ = thread::Builder::new()
            .name("moor-front-egress".to_owned())
            .spawn(move || serve(conn, &denied, &open));
    }
    Ok(0)
}

/// Every address of every interface of the host.
fn host_addresses() -> Result<Vec<IpAddr>, String> {
    let found =
        nix::ifaddrs::getifaddrs().map_err(|e| format!("cannot list the host's addresses: {e}"))?;
    Ok(found
        .filter_map(|ifa| ifa.address)
        .filter_map(|address| {
            address
                .as_sockaddr_in()
                .map(|v4| IpAddr::V4(v4.ip()))
                .or_else(|| address.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip())))
        })
        .collect())
}

/// Answers one CONNECT on `conn`, counting it in `open` while its tunnel lasts.
fn serve(mut conn: TcpStream, denied: &Denied, open: &AtomicUsize) {
    if open.fetch_add(1, Ordering::SeqCst) >= MAX_TUNNELS {
        open.fetch_sub(1, Ordering::SeqCst);
        log("-", 0, "-", "deny", &format!("over-{MAX_TUNNELS}-tunnels"));
        let _ = conn.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let _ = conn.set_read_timeout(Some(TIMEOUT));
    match tunnel(&mut conn, denied) {
        Ok(upstream) => {
            let _ = conn.set_read_timeout(None);
            super::net::splice(conn, upstream);
        }
        Err(status) => {
            let _ = write!(conn, "HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n");
        }
    }
    open.fetch_sub(1, Ordering::SeqCst);
}

/// Reads the request on `conn`, checks it and opens its tunnel, answering 200; otherwise
/// returns the status to answer. Logs the decision either way.
fn tunnel(conn: &mut TcpStream, denied: &Denied) -> Result<TcpStream, &'static str> {
    let head = read_head(conn).map_err(|e| {
        log("-", 0, "-", "deny", &e);
        "400 Bad Request"
    })?;
    let (host, port) = target(&head).ok_or_else(|| {
        log("-", 0, "-", "deny", "not-connect");
        "405 Method Not Allowed"
    })?;
    let deny = |ip: &str, status, reason: &str| {
        log(&host, port, ip, "deny", reason);
        status
    };
    if !ALLOWED_PORTS.contains(&port) {
        return Err(deny("-", "403 Forbidden", "port-not-443-or-80"));
    }
    let addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| deny("-", "502 Bad Gateway", "unresolved"))?
        .collect();
    if let Some((addr, why)) = addrs
        .iter()
        .find_map(|a| denied.why(a.ip()).map(|why| (a, why)))
    {
        return Err(deny(&addr.ip().to_string(), "403 Forbidden", why.word()));
    }
    let ips = addrs
        .iter()
        .map(|a| a.ip().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let upstream = addrs
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, TIMEOUT).ok())
        .ok_or_else(|| deny(&ips, "502 Bad Gateway", "unreachable"))?;
    let at = upstream.peer_addr().map_or(ips, |a| a.ip().to_string());
    log(&host, port, &at, "allow", "public");
    conn.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(|_| "500 Internal Server Error")?;
    Ok(upstream)
}

/// The request head on `conn`, read byte by byte up to its blank line.
fn read_head(conn: &mut TcpStream) -> Result<String, String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err("request head too long".to_owned());
        }
        conn.read_exact(&mut byte).map_err(|e| e.to_string())?;
        head.extend_from_slice(&byte);
    }
    String::from_utf8(head).map_err(|_| "request head is not UTF-8".to_owned())
}

/// The host and port of a `CONNECT host:port` request line; a bracketed IPv6 host loses its
/// brackets.
fn target(head: &str) -> Option<(String, u16)> {
    let mut words = head.lines().next()?.split_whitespace();
    if words.next()? != "CONNECT" {
        return None;
    }
    let (host, port) = words.next()?.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_owned(), port.parse().ok()?))
}

/// One line per decision on stderr, which the controller logs as the front's.
fn log(host: &str, port: u16, ip: &str, decision: &str, reason: &str) {
    let line =
        format!("egress: host={host} port={port} ip={ip} decision={decision} reason={reason}\n");
    let _ = io::stderr().write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{serve, target, MAX_TUNNELS};
    use crate::controller::front_shim::denied::Denied;

    /// What the proxy answers `request` with.
    fn answer(request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let (conn, _) = listener.accept().unwrap();
        serve(conn, &Denied::new(Vec::new()), &AtomicUsize::new(0));
        let mut answer = String::new();
        client.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn any_method_but_connect_is_refused_405_before_the_host_is_resolved() {
        for method in ["GET", "connect", "POST"] {
            let got = answer(&format!("{method} moor-never.invalid:443 HTTP/1.1\r\n\r\n"));
            assert!(
                got.starts_with("HTTP/1.1 405 Method Not Allowed"),
                "{method}: {got}"
            );
        }
        let resolved = answer("CONNECT moor-never.invalid:443 HTTP/1.1\r\n\r\n");
        assert!(
            resolved.starts_with("HTTP/1.1 502 Bad Gateway"),
            "{resolved}"
        );
    }

    #[test]
    fn a_tunnel_past_the_cap_is_answered_429_and_leaves_the_count_alone() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (conn, _) = listener.accept().unwrap();
        let open = AtomicUsize::new(MAX_TUNNELS);
        serve(conn, &Denied::new(Vec::new()), &open);
        let mut answer = String::new();
        client.read_to_string(&mut answer).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 429 Too Many Requests"),
            "{answer}"
        );
        assert_eq!(open.load(Ordering::SeqCst), MAX_TUNNELS);
    }

    #[test]
    fn a_connect_line_names_its_host_and_port() {
        assert_eq!(
            target("CONNECT slack.com:443 HTTP/1.1\r\nHost: slack.com:443\r\n\r\n"),
            Some(("slack.com".to_owned(), 443))
        );
        assert_eq!(
            target("CONNECT [2606:4700::1111]:443 HTTP/1.1\r\n\r\n"),
            Some(("2606:4700::1111".to_owned(), 443))
        );
        assert_eq!(target("GET http://slack.com/ HTTP/1.1\r\n\r\n"), None);
        assert_eq!(target("CONNECT slack.com HTTP/1.1\r\n\r\n"), None);
    }
}
