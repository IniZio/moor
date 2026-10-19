//! Listener placed in front of the embedded nono proxy; it passes CONNECT and
//! plain-HTTP proxy requests, both as a CONNECT.
//!
//! nono also serves origin-form requests (its reverse mode), which dial the
//! upstream directly with the credential injected and skip the external
//! proxy, and so skip moor's GraphQL filter. This listener forwards a CONNECT,
//! turns an absolute-form `http://` request into one (see `plain`), and
//! refuses every other request before nono sees a byte of it.
//!
//! The Box authenticates to this listener with its own nonce. The listener
//! then swaps that credential for an internal token that only the supervisor
//! and nono know, so a request the Box loops back into nono's own port cannot
//! present anything nono accepts.
//!
//! Request bytes carry credentials, so nothing here logs them.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::Secret;

mod auth;
mod plain;

/// Largest request or response head accepted.
const MAX_HEAD: usize = 8 * 1024;
/// Most reads spent on one head, so a slow drip cannot hold a thread forever.
const MAX_READS: usize = 512;
/// Read timeout while the client sends its request head.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Connect, write and response-head timeout toward nono.
const TARGET_TIMEOUT: Duration = Duration::from_secs(30);
/// Idle timeout per direction once the tunnel is established.
const TUNNEL_IDLE: Duration = Duration::from_secs(600);
/// Time spent draining a refused client so it reads the 403.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// Most bytes drained from a refused client.
const DRAIN_LIMIT: u64 = 64 * 1024;
/// Body of the refusal.
const REFUSAL_BODY: &str = r#"{"message":"moor: only CONNECT or http:// requests are allowed"}"#;
/// Body of the credential refusal.
const AUTH_BODY: &str = r#"{"message":"moor: proxy authentication required"}"#;

/// The two credentials: what the Box presents and what nono checks.
struct Creds {
    /// Credential the Box presents.
    box_nonce: Secret,
    /// Session token nono checks; never shown to the Box.
    internal: Secret,
}

/// Listener on 127.0.0.1 that passes only CONNECT requests to `target` (nono).
pub(crate) struct Front {
    /// Port the listener is bound to.
    port: u16,
    /// Set to ask the acceptor to exit.
    stop: Arc<AtomicBool>,
    /// Acceptor thread.
    acceptor: JoinHandle<()>,
}

impl fmt::Debug for Front {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Front")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Front {
    /// Binds 127.0.0.1 on an ephemeral port and starts accepting. `box_nonce`
    /// is the credential the Box presents; `internal` is the session token
    /// nono checks, never shown to the Box.
    pub(crate) fn start(
        target: SocketAddr,
        box_nonce: Secret,
        internal: Secret,
    ) -> io::Result<Self> {
        let creds = Arc::new(Creds {
            box_nonce,
            internal,
        });
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let acceptor = thread::Builder::new()
            .name("moor-front".to_owned())
            .spawn(move || accept_loop(&listener, &flag, target, &creds))?;
        Ok(Self {
            port,
            stop,
            acceptor,
        })
    }

    /// Port the listener is bound to.
    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    /// Stops accepting and joins the acceptor. Open connections end on their
    /// socket timeouts or when a peer closes.
    pub(crate) fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)),
            DRAIN_TIMEOUT,
        );
        let _ = self.acceptor.join();
    }
}

/// Accepts connections until `stop` is set, one thread per connection.
fn accept_loop(listener: &TcpListener, stop: &AtomicBool, target: SocketAddr, creds: &Arc<Creds>) {
    for incoming in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match incoming {
            Ok(client) => {
                let creds = Arc::clone(creds);
                let _ = thread::Builder::new()
                    .name("moor-front-conn".to_owned())
                    .spawn(move || serve(client, target, &creds));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// A message head and the bytes read past it.
struct Head {
    /// Bytes up to and including the blank line.
    bytes: Vec<u8>,
    /// Bytes that followed the blank line in the same reads.
    rest: Vec<u8>,
}

/// Reads up to the blank line ending a head; `None` when the head is
/// oversize, the peer closes first, or a read fails.
fn read_head(src: &mut impl Read) -> Option<Head> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 2048];
    for _ in 0..MAX_READS {
        let n = src.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(chunk.get(..n)?);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let end = pos + 4;
            if end > MAX_HEAD {
                return None;
            }
            let rest = buf.split_off(end);
            return Some(Head { bytes: buf, rest });
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
    None
}

/// Handles one client connection.
fn serve(mut client: TcpStream, target: SocketAddr, creds: &Creds) {
    let _ = client.set_read_timeout(Some(REQUEST_TIMEOUT));
    let _ = client.set_write_timeout(Some(TARGET_TIMEOUT));
    let Some(head) = read_head(&mut client) else {
        return refuse(client);
    };
    if !is_connect(&head.bytes) {
        return serve_plain(client, target, creds, &head);
    }
    let Some(rewritten) = auth::authorize(&head.bytes, &creds.box_nonce, &creds.internal) else {
        return deny_auth(client);
    };
    let Some(upstream) = open_upstream(target, &rewritten) else {
        return refuse(client);
    };
    relay(client, upstream, &Entry::Connect, &head.rest);
}

/// Handles an absolute-form `http://` request as a CONNECT to nono; every
/// other non-CONNECT request is refused.
fn serve_plain(client: TcpStream, target: SocketAddr, creds: &Creds, head: &Head) {
    let Some(plain) = plain::parse(&head.bytes) else {
        return refuse(client);
    };
    if auth::authorize(&head.bytes, &creds.box_nonce, &creds.internal).is_none() {
        return deny_auth(client);
    }
    let connect = plain::connect_head(&plain.authority, creds.internal.expose());
    let Some(upstream) = open_upstream(target, &connect) else {
        return refuse(client);
    };
    relay(client, upstream, &Entry::Plain(plain.request), &head.rest);
}

/// How the client entered the tunnel.
enum Entry {
    /// A CONNECT: nono's answer goes to the client.
    Connect,
    /// A plain request: the bytes to write into the tunnel once nono accepts.
    Plain(Vec<u8>),
}

/// Connects to nono and writes the rewritten head.
fn open_upstream(target: SocketAddr, head: &[u8]) -> Option<TcpStream> {
    let mut upstream = TcpStream::connect_timeout(&target, TARGET_TIMEOUT).ok()?;
    upstream.set_read_timeout(Some(TARGET_TIMEOUT)).ok()?;
    upstream.set_write_timeout(Some(TARGET_TIMEOUT)).ok()?;
    upstream.write_all(head).ok()?;
    Some(upstream)
}

/// Relays nono's answer, then splices the tunnel when nono accepted it.
/// Client bytes sent after the head stay buffered until nono answers 2xx, so
/// a refused CONNECT never leaves nono a second request on the connection.
fn relay(mut client: TcpStream, mut upstream: TcpStream, entry: &Entry, client_rest: &[u8]) {
    let Some(answer) = read_head(&mut upstream) else {
        return refuse(client);
    };
    let success = is_success(&answer.bytes);
    if (!success || matches!(entry, Entry::Connect)) && client.write_all(&answer.bytes).is_err() {
        return;
    }
    if !success {
        let _ = client.write_all(&answer.rest);
        let _ = client.shutdown(Shutdown::Write);
        return;
    }
    if let Entry::Plain(request) = entry {
        if upstream.write_all(request).is_err() {
            return;
        }
    }
    if client.write_all(&answer.rest).is_err() || upstream.write_all(client_rest).is_err() {
        return;
    }
    splice(client, upstream);
}

/// Copies both directions until each side ends or idles out.
fn splice(client: TcpStream, upstream: TcpStream) {
    let _ = client.set_read_timeout(Some(TUNNEL_IDLE));
    let _ = upstream.set_read_timeout(Some(TUNNEL_IDLE));
    let (Ok(client_rx), Ok(upstream_tx)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    let uplink = thread::Builder::new()
        .name("moor-front-up".to_owned())
        .spawn(move || pump(client_rx, upstream_tx));
    pump(upstream, client);
    if let Ok(handle) = uplink {
        let _ = handle.join();
    }
}

/// Copies `from` to `to`, then closes the write half of `to`.
fn pump(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
}

/// Answers 403 and closes so the client reads the reply before the close.
fn refuse(mut client: TcpStream) {
    let reply = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{REFUSAL_BODY}",
        REFUSAL_BODY.len()
    );
    finish_refusal(&mut client, &reply);
}

/// Writes `reply`, closes the write half and drains what the client still sends.
fn finish_refusal(client: &mut TcpStream, reply: &str) {
    let _ = client.write_all(reply.as_bytes());
    let _ = client.shutdown(Shutdown::Write);
    let _ = client.set_read_timeout(Some(DRAIN_TIMEOUT));
    let _ = io::copy(&mut (&mut *client).take(DRAIN_LIMIT), &mut io::sink());
}

/// Answers 407 and closes the same way a refusal does.
fn deny_auth(mut client: TcpStream) {
    let reply = format!(
        "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"moor\"\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{AUTH_BODY}",
        AUTH_BODY.len()
    );
    finish_refusal(&mut client, &reply);
}

/// True when nono's status line carries a 2xx code.
fn is_success(head: &[u8]) -> bool {
    head.starts_with(b"HTTP/1.") && head.get(8) == Some(&b' ') && head.get(9) == Some(&b'2')
}

/// True for a well-formed `CONNECT host:port HTTP/1.x` head with well-formed
/// header lines. Only this shape passes, so origin-form and absolute-form
/// requests never reach nono's reverse and forward handlers.
fn is_connect(head: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let Some(block) = text.strip_suffix("\r\n\r\n") else {
        return false;
    };
    let mut lines = block.split("\r\n");
    lines.next().is_some_and(request_line_ok) && lines.all(header_ok)
}

/// True for exactly `CONNECT <authority> HTTP/1.1` (or 1.0), single-spaced.
fn request_line_ok(line: &str) -> bool {
    let mut parts = line.split(' ');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some("CONNECT"), Some(authority), Some("HTTP/1.1" | "HTTP/1.0"), None)
            if authority_ok(authority)
    )
}

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
