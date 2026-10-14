//! Loopback listener: reads a request head, runs the gate, answers rejects
//! itself and hands admitted requests to a pluggable handler.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::proxy_gate::{decide, GateDecision, GateInput, Plan, RouteId, RouteTable};
use moor_core::seam::clock::Clock;
use moor_core::Secret;

use super::head::{read_head, Head};
use super::ProxyLog;

mod deadline;
mod framing;

pub(crate) use deadline::DeadlineReader;
pub(crate) use framing::body_cap;
use framing::BodyLen;

/// Total time a client gets to send a complete request head, however slowly
/// it sends the bytes.
pub(crate) const HEAD_BUDGET: Duration = Duration::from_secs(10);

/// Longest a client may stall mid-body before the handler gives up.
const BODY_IDLE: Duration = Duration::from_secs(10);

/// Pause after a failed `accept`. Persistent failures such as fd exhaustion
/// return at once, so without it the accept thread would spin a core.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

/// Most unread request bytes discarded after a refusal.
const DRAIN_LIMIT: u64 = 64 * 1024;
/// Total time spent discarding unread request bytes after a refusal.
const DRAIN_BUDGET: Duration = Duration::from_secs(1);

/// Per-listener settings.
pub(crate) struct ProxyOpts {
    /// Most connections served at once.
    pub(crate) max_conns: usize,
    /// The Task repo (`owner/name`) forge calls are limited to.
    pub(crate) task_repo: Option<String>,
    /// Source of time for the head deadline.
    pub(crate) clock: Arc<dyn Clock>,
    /// Total time allowed for reading one request head.
    pub(crate) head_budget: Duration,
    /// Largest declared request body accepted on a route, in bytes.
    pub(crate) body_cap: fn(&RouteId) -> u64,
}

/// An admitted request, ready to be sent upstream.
pub(crate) struct Forward {
    /// The parsed request head.
    pub(crate) head: Head,
    /// What the gate decided to send upstream.
    pub(crate) plan: Plan,
    /// Bytes already read past the head. These may run beyond the declared
    /// body (pipelined data): the handler must cut the body to its declared
    /// framing, `Content-Length` or chunked, and ignore anything after it.
    ///
    /// A chunked body has no declared length, so the handler must enforce
    /// the same per-route size cap while streaming it.
    pub(crate) body_prefix: Vec<u8>,
}

/// Completes an admitted request.
pub(crate) trait ForwardHandler: Send + Sync {
    /// Writes the full response to `stream` and returns its status for the log.
    fn forward(&self, req: Forward, stream: &mut TcpStream) -> u16;
}

/// State shared by the accept thread and every connection thread.
struct Shared {
    /// Routes the gate admits.
    table: RouteTable,
    /// Per-listener settings.
    opts: ProxyOpts,
    /// Per-Task secret clients must present.
    nonce: Secret,
    /// One line per request.
    log: ProxyLog,
    /// Receives admitted requests.
    handler: Arc<dyn ForwardHandler>,
    /// Connections currently being served.
    active: AtomicUsize,
}

/// Holds one connection slot until dropped.
struct Slot(Arc<Shared>);

impl Slot {
    /// Takes a slot, or returns `None` when the cap is reached.
    fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        let before = shared.active.fetch_add(1, Ordering::SeqCst);
        let slot = Self(Arc::clone(shared));
        (before < shared.opts.max_conns).then_some(slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A running loopback proxy.
pub(crate) struct Proxy {
    /// Bound loopback port.
    port: u16,
    /// Asks the accept thread to exit.
    stop: Arc<AtomicBool>,
    /// The accept thread.
    accept: JoinHandle<()>,
}

impl Proxy {
    /// Binds a loopback port and starts accepting connections.
    pub(crate) fn start(
        table: RouteTable,
        nonce: Secret,
        log: ProxyLog,
        handler: Arc<dyn ForwardHandler>,
        opts: ProxyOpts,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            table,
            opts,
            nonce,
            log,
            handler,
            active: AtomicUsize::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let accept = thread::Builder::new()
            .name("moor-proxy-accept".to_owned())
            .spawn(move || accept_loop(&listener, &shared, &flag))?;
        Ok(Self { port, stop, accept })
    }

    /// Returns the bound loopback port.
    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    /// Stops accepting and waits for the accept thread.
    pub(crate) fn shutdown(self) {
        self.stop.store(true, Ordering::SeqCst);
        drop(TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)));
        drop(self.accept.join());
    }
}

/// Accepts connections until told to stop.
fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>, stop: &AtomicBool) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            thread::sleep(ACCEPT_BACKOFF);
            continue;
        };
        let Some(slot) = Slot::acquire(shared) else {
            let mut stream = stream;
            write_reject(&mut stream, 503);
            continue;
        };
        let shared = Arc::clone(shared);
        let spawned = thread::Builder::new()
            .name("moor-proxy-conn".to_owned())
            .spawn(move || {
                let _slot = slot;
                serve(&shared, stream);
            });
        drop(spawned);
    }
}

/// Serves one connection.
fn serve(shared: &Shared, mut stream: TcpStream) {
    let Some((head, body_prefix)) = read_request_head(shared, &mut stream) else {
        return;
    };
    let Ok(len) = framing::body_len(&head.headers) else {
        reject_unread(shared, &mut stream, &head.method, "-", 400);
        return;
    };
    let decision = decide(&GateInput {
        table: &shared.table,
        method: &head.method,
        target: &head.target,
        headers: &head.header_pairs(),
        nonce: &shared.nonce,
        task_repo: shared.opts.task_repo.as_deref(),
    });
    match decision {
        GateDecision::Reject(reject) => {
            let status = reject.status();
            write_reject(&mut stream, status);
            shared.log.record(&head.method, "-", status);
        }
        GateDecision::Forward(plan) if over_cap(shared, &plan, len) => {
            reject_unread(shared, &mut stream, &head.method, plan.route.as_str(), 413);
        }
        GateDecision::Forward(plan) => hand_off(shared, &mut stream, head, plan, body_prefix),
    }
}

/// Tells whether the declared body is larger than the route allows.
fn over_cap(shared: &Shared, plan: &Plan, len: BodyLen) -> bool {
    matches!(len, BodyLen::Length(n) if n > (shared.opts.body_cap)(&plan.route))
}

/// Reads the request head within the head budget, answering malformed heads.
fn read_request_head(shared: &Shared, stream: &mut TcpStream) -> Option<(Head, Vec<u8>)> {
    let mut reader =
        DeadlineReader::new(stream, shared.opts.clock.as_ref(), shared.opts.head_budget);
    match read_head(&mut reader) {
        Ok(read) => read,
        Err(err) => {
            if let Some(status) = err.status() {
                write_reject(stream, status);
                shared.log.record("-", "-", status);
            }
            None
        }
    }
}

/// Passes an admitted request to the handler and logs the outcome.
fn hand_off(shared: &Shared, stream: &mut TcpStream, head: Head, plan: Plan, body_prefix: Vec<u8>) {
    drop(stream.set_read_timeout(Some(BODY_IDLE)));
    if head.expects_continue() {
        drop(stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n"));
    }
    let method = head.method.clone();
    let route = plan.route.as_str().to_owned();
    let req = Forward {
        head,
        plan,
        body_prefix,
    };
    let status = shared.handler.forward(req, stream);
    shared.log.record(&method, &route, status);
}

/// Answers and logs a request refused before its body was read, then drops
/// the unread part of what the client already sent so closing the socket
/// does not reset the connection before the client reads the answer.
fn reject_unread(shared: &Shared, stream: &mut TcpStream, method: &str, route: &str, status: u16) {
    write_reject(stream, status);
    shared.log.record(method, route, status);
    drain(shared, stream);
}

/// Discards up to [`DRAIN_LIMIT`] bytes the client has sent, stopping when it
/// goes quiet or [`DRAIN_BUDGET`] has passed in total.
fn drain(shared: &Shared, stream: &TcpStream) {
    let reader = DeadlineReader::new(stream, shared.opts.clock.as_ref(), DRAIN_BUDGET);
    drop(io::copy(&mut reader.take(DRAIN_LIMIT), &mut io::sink()));
}

/// Writes a fixed-body reject response and closes the write side.
fn write_reject(stream: &mut TcpStream, status: u16) {
    let (reason, body) = match status {
        400 => ("Bad Request", "bad request\n"),
        401 => ("Unauthorized", "unauthorized\n"),
        403 => ("Forbidden", "forbidden\n"),
        404 => ("Not Found", "not found\n"),
        405 => ("Method Not Allowed", "method not allowed\n"),
        431 => (
            "Request Header Fields Too Large",
            "request header fields too large\n",
        ),
        413 => ("Content Too Large", "content too large\n"),
        503 => ("Service Unavailable", "busy\n"),
        _ => ("Error", "error\n"),
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    drop(stream.write_all(response.as_bytes()));
    drop(stream.shutdown(Shutdown::Write));
}

#[cfg(test)]
mod tests;
