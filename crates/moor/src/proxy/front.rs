//! Listener placed in front of the embedded nono proxy; it passes CONNECT and
//! plain-HTTP proxy requests, both as a CONNECT.
//!
//! nono also serves origin-form requests (its reverse mode), which dial the
//! upstream directly with the credential injected and skip the external
//! proxy, and so skip moor's GraphQL filter. This listener forwards a CONNECT,
//! turns an absolute-form `http://` request into one (see `plain`), and
//! refuses every other request before nono sees a byte of it.
//!
//! It listens on a Unix socket that only moor's user can open (see `socket`).
//!
//! It replaces any proxy credential the Box sends with an internal token that only the
//! supervisor and nono know, so a request the Box loops back into nono's own port cannot
//! present anything nono accepts.
//!
//! Request bytes carry credentials, so nothing here logs them.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::Secret;

pub(crate) use machine::{Held, MachineRoute};
pub(crate) use socket::FrontSocket;
use socket::{Handler, Listening};

mod auth;
mod machine;
mod plain;
mod socket;

/// Largest request or response head accepted.
const MAX_HEAD: usize = 8 * 1024;
/// Most reads spent on one head, so a slow drip cannot hold a thread forever.
const MAX_READS: usize = 512;
/// Read timeout while the client sends its request head.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Connect, write and response-head timeout toward nono.
const TARGET_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(30)
};
/// Idle timeout per direction once the tunnel is established.
const TUNNEL_IDLE: Duration = Duration::from_secs(600);
/// Time spent draining a refused client so it reads the 403.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// Most bytes drained from a refused client.
const DRAIN_LIMIT: u64 = 64 * 1024;
/// Body of the refusal.
const REFUSAL_BODY: &str = r#"{"message":"moor: only CONNECT or http:// requests are allowed"}"#;

/// Picks the nono a request head goes to and the internal token it presents there; `None`
/// refuses the request.
pub(crate) type Route = Arc<RouteFn>;

/// What a [`Route`] calls.
pub(crate) type RouteFn = dyn Fn(&[u8]) -> Option<(SocketAddr, Secret)> + Send + Sync;

/// Listener that passes only CONNECT requests to nono.
pub(crate) struct Front {
    /// Where it listens.
    socket: FrontSocket,
    /// Serves each accepted connection.
    handler: Handler,
    /// The bound socket, replaced when the file must be bound again.
    listening: Mutex<Listening>,
}

impl fmt::Debug for Front {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Front")
            .field("path", &self.socket.path())
            .finish_non_exhaustive()
    }
}

impl Front {
    /// Binds `socket`, replacing a stale file, and starts accepting. `internal`
    /// is the session token nono checks, never shown to the Box; `machine` is the service
    /// the front serves itself.
    pub(crate) fn start(
        target: SocketAddr,
        socket: FrontSocket,
        internal: Secret,
        machine: Option<MachineRoute>,
    ) -> Result<Self, String> {
        Self::bind(
            socket,
            Arc::new(move |_| Some((target, Secret::new(internal.expose().to_vec())))),
            machine,
        )
    }

    /// Binds `socket`, replacing a stale file, and starts accepting; `route` picks each
    /// request's nono from its head.
    pub(crate) fn routed(socket: FrontSocket, route: Route) -> Result<Self, String> {
        Self::bind(socket, route, None)
    }

    /// Binds `socket` and serves each connection by `route` and `machine`.
    fn bind(
        socket: FrontSocket,
        route: Route,
        machine: Option<MachineRoute>,
    ) -> Result<Self, String> {
        let handler: Handler = Arc::new(move |client| serve(client, &*route, machine.as_ref()));
        let listening = socket.listen(true, &handler)?;
        Ok(Self {
            socket,
            handler,
            listening: Mutex::new(listening),
        })
    }

    /// The socket path.
    pub(crate) fn path(&self) -> &Path {
        self.socket.path()
    }

    /// Binds the socket again when its file is gone.
    pub(crate) fn rebind(&self) -> Result<(), String> {
        if !self.socket.gone() {
            return Ok(());
        }
        let fresh = self.socket.listen(false, &self.handler)?;
        let old = std::mem::replace(
            &mut *self
                .listening
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            fresh,
        );
        old.halt();
        Ok(())
    }

    /// Stops accepting and removes the socket file while it is still this front's.
    pub(crate) fn stop(self) {
        let listening = self
            .listening
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        self.socket.close(listening);
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
fn serve(mut client: UnixStream, route: &RouteFn, machine: Option<&MachineRoute>) {
    let _ = client.set_read_timeout(Some(REQUEST_TIMEOUT));
    let _ = client.set_write_timeout(Some(TARGET_TIMEOUT));
    let Some(head) = read_head(&mut client) else {
        return refuse(client);
    };
    if let Some(machine) = machine.filter(|m| m.aims_at(&head.bytes)) {
        return machine.serve(client, &head);
    }
    let Some((target, internal)) = route(&head.bytes) else {
        return refuse(client);
    };
    let internal = &internal;
    if !is_connect(&head.bytes) {
        return serve_plain(client, target, internal, &head);
    }
    let Some(rewritten) = auth::with_token(&head.bytes, internal) else {
        return refuse(client);
    };
    let Some(upstream) = open_upstream(target, &rewritten) else {
        return refuse(client);
    };
    relay(client, upstream, &Entry::Connect, &head.rest);
}

/// Handles an absolute-form `http://` request as a CONNECT to nono; every
/// other non-CONNECT request is refused.
fn serve_plain(client: UnixStream, target: SocketAddr, internal: &Secret, head: &Head) {
    let Some(plain) = plain::parse(&head.bytes) else {
        return refuse(client);
    };
    let connect = plain::connect_head(&plain.authority, internal.expose());
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
fn relay(mut client: UnixStream, mut upstream: TcpStream, entry: &Entry, client_rest: &[u8]) {
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
fn splice(mut client: UnixStream, mut upstream: TcpStream) {
    let _ = client.set_read_timeout(Some(TUNNEL_IDLE));
    let _ = upstream.set_read_timeout(Some(TUNNEL_IDLE));
    let (Ok(mut client_rx), Ok(mut upstream_tx)) = (client.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let uplink = thread::Builder::new()
        .name("moor-front-up".to_owned())
        .spawn(move || {
            let _ = io::copy(&mut client_rx, &mut upstream_tx);
            let _ = upstream_tx.shutdown(Shutdown::Write);
        });
    let _ = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    if let Ok(handle) = uplink {
        let _ = handle.join();
    }
}

/// Answers 403, closes the write half and drains what the client still sends, so
/// the client reads the reply before the close.
fn refuse(mut client: UnixStream) {
    let reply = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{REFUSAL_BODY}",
        REFUSAL_BODY.len()
    );
    let _ = client.write_all(reply.as_bytes());
    let _ = client.shutdown(Shutdown::Write);
    let _ = client.set_read_timeout(Some(DRAIN_TIMEOUT));
    let _ = io::copy(&mut (&mut client).take(DRAIN_LIMIT), &mut io::sink());
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
