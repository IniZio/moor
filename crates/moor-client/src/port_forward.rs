//! A person's computer reaches a port on a Box's loopback: a listener on the computer's own
//! loopback, and one controller connection per TCP connection it accepts.
//!
//! The local port is the Box's port unless the caller picks another, since the app may name it
//! in a sign-in redirect. A listener binds `127.0.0.1` and, where the computer has it, `[::1]`,
//! because a browser may resolve `localhost` to either.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use moor_core::request::{BoxTarget, ForwardCloseReq, ForwardOpenReq, ForwardReq, Request};
use moor_core::BoxName;

use super::Client;

/// Most connections one forward carries at once.
pub const MAX_CONNECTIONS: u64 = 64;
/// How often a listener checks whether the forward was told to end.
const POLL: Duration = Duration::from_millis(50);
/// How long a forward that was told to end waits for its connections to finish.
const DRAIN: Duration = Duration::from_secs(2);
/// How often a draining forward checks whether its connections finished.
const DRAIN_POLL: Duration = Duration::from_millis(10);
/// The controller's words for a Box that closed, which end the forward.
const CLOSED: &str = "is closed; the forward ended";

/// What to forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The Box's name.
    pub box_name: String,
    /// The port on the Box's loopback.
    pub port: u16,
    /// The port on this computer; the Box's port when `None`.
    pub local_port: Option<u16>,
}

/// What a forward reports as it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The forward is open.
    Opened {
        /// The port on this computer.
        local_port: u16,
    },
    /// Connection `n` started.
    ConnectionOpen(u64),
    /// Connection `n` ended, after carrying these bytes.
    ConnectionClosed {
        /// Which connection.
        n: u64,
        /// Bytes from this computer into the Box.
        bytes_in: u64,
        /// Bytes from the Box back to this computer.
        bytes_out: u64,
    },
    /// A connection was dropped because [`MAX_CONNECTIONS`] are already open.
    Full,
    /// Connection `n` could not be carried, for this reason.
    ConnectionRefused {
        /// Which connection.
        n: u64,
        /// The controller's words.
        reason: String,
    },
}

impl Event {
    /// The line that tells a person about this event, without a time.
    #[must_use]
    pub fn describe(&self, spec: &Spec) -> String {
        match self {
            Self::Opened { local_port } => format!(
                "http://localhost:{local_port}  →  port {} in {}. Ctrl-C ends the forward.",
                spec.port, spec.box_name
            ),
            Self::ConnectionOpen(n) => format!("connection {n} open"),
            Self::Full => format!("{MAX_CONNECTIONS} connections are open; one more was dropped"),
            Self::ConnectionClosed {
                n,
                bytes_in,
                bytes_out,
            } => format!(
                "connection {n} closed, {} in, {} out",
                kib(*bytes_in),
                kib(*bytes_out)
            ),
            Self::ConnectionRefused { n, reason } => format!("connection {n} refused: {reason}"),
        }
    }
}

/// `bytes` in KiB, rounded up.
fn kib(bytes: u64) -> String {
    format!("{} KiB", bytes.div_ceil(1024))
}

/// What a forward carried in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    /// Connections carried.
    pub connections: u64,
    /// Bytes from this computer into the Box.
    pub bytes_in: u64,
    /// Bytes from the Box back to this computer.
    pub bytes_out: u64,
}

/// Why a forward could not start or ended early.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Where a forward's events go.
pub type Say = Arc<dyn Fn(Event) + Send + Sync>;

/// The state every connection of one forward shares.
struct Forward {
    /// The controller.
    client: Client,
    /// The Box.
    target: BoxTarget,
    /// The port on the Box's loopback.
    port: u16,
    /// Set to end the forward.
    stop: Arc<AtomicBool>,
    /// Set once the Box closed.
    ended: AtomicBool,
    /// Where events go.
    say: Say,
    /// Connections started.
    connections: AtomicU64,
    /// Bytes from this computer into the Box, of connections that ended.
    bytes_in: AtomicU64,
    /// Bytes from the Box back to this computer, of connections that ended.
    bytes_out: AtomicU64,
    /// The connections in flight, by number.
    live: Mutex<HashMap<u64, TcpStream>>,
}

impl Forward {
    /// Whether the forward was told to end or its Box closed.
    fn over(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.ended.load(Ordering::SeqCst)
    }

    /// The connections in flight.
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<u64, TcpStream>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Accepts connections on `listener` until the forward is over.
    fn accept(self: &Arc<Self>, listener: &TcpListener) {
        while !self.over() {
            match listener.accept() {
                Ok((conn, _)) => self.start(conn),
                Err(_) => thread::sleep(POLL),
            }
        }
    }

    /// Carries `conn` on a thread of its own, unless the forward is full.
    fn start(self: &Arc<Self>, conn: TcpStream) {
        let _ = conn.set_nonblocking(false);
        let Ok(mine) = conn.try_clone() else { return };
        let n = {
            let mut live = self.live();
            if live.len() as u64 >= MAX_CONNECTIONS {
                drop(live);
                (self.say)(Event::Full);
                return;
            }
            let n = self.connections.fetch_add(1, Ordering::SeqCst) + 1;
            live.insert(n, mine);
            n
        };
        let this = Arc::clone(self);
        thread::spawn(move || {
            this.carry(n, &conn);
            this.live().remove(&n);
        });
    }

    /// Carries connection `conn`, number `n`, through the controller.
    fn carry(&self, n: u64, conn: &TcpStream) {
        (self.say)(Event::ConnectionOpen(n));
        let (bytes_in, bytes_out) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (Ok(reader), Ok(writer)) = (conn.try_clone(), conn.try_clone()) else {
            return;
        };
        let request = Request::Forward(ForwardReq {
            target: self.target.clone(),
            port: self.port,
        });
        let up = Counting {
            inner: reader,
            n: Arc::clone(&bytes_in),
        };
        let mut down = Counting {
            inner: writer,
            n: Arc::clone(&bytes_out),
        };
        let answer = self.client.pipe(request, up, &mut down, &mut io::sink());
        let _ = conn.shutdown(Shutdown::Both);
        let (up, down) = (
            bytes_in.load(Ordering::SeqCst),
            bytes_out.load(Ordering::SeqCst),
        );
        self.bytes_in.fetch_add(up, Ordering::SeqCst);
        self.bytes_out.fetch_add(down, Ordering::SeqCst);
        let refused = match answer {
            Ok(resp) => resp.error.map(|e| e.message),
            Err(e) => Some(e.to_string()),
        };
        match refused {
            Some(reason) => {
                if reason.ends_with(CLOSED) {
                    self.ended.store(true, Ordering::SeqCst);
                }
                (self.say)(Event::ConnectionRefused { n, reason });
            }
            None => (self.say)(Event::ConnectionClosed {
                n,
                bytes_in: up,
                bytes_out: down,
            }),
        }
    }

    /// Cuts what is still open and waits a moment for it to finish.
    fn drain(&self) {
        for conn in self.live().values() {
            let _ = conn.shutdown(Shutdown::Both);
        }
        for _ in 0..(DRAIN.as_millis() / DRAIN_POLL.as_millis()) {
            if self.live().is_empty() {
                return;
            }
            thread::sleep(DRAIN_POLL);
        }
    }
}

/// Forwards `spec` until `stop` is set or the Box closes, then tells the controller what it
/// carried. `say` hears each event, from any thread.
///
/// # Errors
/// The Box name is not one, the local port is taken, the controller refuses the forward, or the
/// Box closed.
pub fn run(
    client: &Client,
    spec: &Spec,
    stop: &Arc<AtomicBool>,
    say: Say,
) -> Result<Counts, Error> {
    let name = BoxName::parse_allow_bare(&spec.box_name).map_err(|e| Error(e.to_string()))?;
    let target = BoxTarget::Name(name);
    let (listeners, local_port) = bind(spec.local_port.unwrap_or(spec.port))?;
    let open = Request::ForwardOpen(ForwardOpenReq {
        target: target.clone(),
        port: spec.port,
        local_port,
    });
    let answer = client
        .request(open, |_| {})
        .map_err(|e| Error(e.to_string()))?;
    if let Some(e) = answer.error {
        return Err(Error(e.message));
    }
    say(Event::Opened { local_port });
    let fwd = Arc::new(Forward {
        client: client.clone(),
        target: target.clone(),
        port: spec.port,
        stop: Arc::clone(stop),
        ended: AtomicBool::new(false),
        say,
        connections: AtomicU64::new(0),
        bytes_in: AtomicU64::new(0),
        bytes_out: AtomicU64::new(0),
        live: Mutex::new(HashMap::new()),
    });
    thread::scope(|s| {
        for listener in &listeners {
            let fwd = &fwd;
            s.spawn(move || fwd.accept(listener));
        }
    });
    fwd.drain();
    let counts = Counts {
        connections: fwd.connections.load(Ordering::SeqCst),
        bytes_in: fwd.bytes_in.load(Ordering::SeqCst),
        bytes_out: fwd.bytes_out.load(Ordering::SeqCst),
    };
    let close = Request::ForwardClose(ForwardCloseReq {
        target,
        port: spec.port,
        connections: counts.connections,
        bytes_in: counts.bytes_in,
        bytes_out: counts.bytes_out,
    });
    let _ = client.request(close, |_| {});
    if fwd.ended.load(Ordering::SeqCst) {
        return Err(Error(format!("{} {CLOSED}", spec.box_name)));
    }
    Ok(counts)
}

/// Binds the loopback listeners for `port`, which may be 0 for any free one: IPv4 must bind, and
/// IPv6 binds where the computer has it. A port held on either is taken; an IPv6 loopback the
/// computer lacks is skipped. The listeners and the port they hold.
fn bind(port: u16) -> Result<(Vec<TcpListener>, u16), Error> {
    let taken = |port: u16| {
        Error(format!(
            "localhost:{port} is in use on this computer; stop what holds it or pass --local-port"
        ))
    };
    let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .map_err(|_| taken(port))?;
    let port = v4.local_addr().map_err(|e| Error(e.to_string()))?.port();
    let mut all = vec![v4];
    match TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))) {
        Ok(v6) => all.push(v6),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Err(taken(port)),
        Err(_) => {}
    }
    for l in &all {
        l.set_nonblocking(true).map_err(|e| Error(e.to_string()))?;
    }
    Ok((all, port))
}

/// A stream whose bytes are counted.
struct Counting {
    /// The stream.
    inner: TcpStream,
    /// Bytes moved.
    n: Arc<AtomicU64>,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.n.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.n.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
