//! Loopback listener: reads a request head, runs the gate, answers rejects
//! itself and hands admitted requests to a pluggable handler.

use std::io::{self, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::proxy_gate::{decide, GateDecision, GateInput, Plan, RouteTable};
use moor_core::Secret;

use super::head::{read_head, Head};
use super::ProxyLog;

/// How long a connection may stall while sending its request head.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// An admitted request, ready to be sent upstream.
pub(crate) struct Forward {
    /// The parsed request head.
    pub(crate) head: Head,
    /// What the gate decided to send upstream.
    pub(crate) plan: Plan,
    /// Body bytes already read past the head.
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
    /// The Task repo (`owner/name`) forge calls are limited to.
    task_repo: Option<String>,
    /// Per-Task secret clients must present.
    nonce: Secret,
    /// One line per request.
    log: ProxyLog,
    /// Receives admitted requests.
    handler: Arc<dyn ForwardHandler>,
    /// Connections currently being served.
    active: AtomicUsize,
    /// Most connections served at once.
    max_conns: usize,
}

/// Holds one connection slot until dropped.
struct Slot(Arc<Shared>);

impl Slot {
    /// Takes a slot, or returns `None` when the cap is reached.
    fn acquire(shared: &Arc<Shared>) -> Option<Self> {
        let before = shared.active.fetch_add(1, Ordering::SeqCst);
        let slot = Self(Arc::clone(shared));
        (before < shared.max_conns).then_some(slot)
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
        max_conns: usize,
        task_repo: Option<String>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            table,
            task_repo,
            nonce,
            log,
            handler,
            active: AtomicUsize::new(0),
            max_conns,
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
        let Ok(stream) = stream else { continue };
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
    drop(stream.set_read_timeout(Some(READ_TIMEOUT)));
    let (head, body_prefix) = match read_head(&mut stream) {
        Ok(Some(read)) => read,
        Ok(None) => return,
        Err(err) => {
            if let Some(status) = err.status() {
                write_reject(&mut stream, status);
                shared.log.record("-", "-", status);
            }
            return;
        }
    };
    let decision = decide(&GateInput {
        table: &shared.table,
        method: &head.method,
        target: &head.target,
        headers: &head.header_pairs(),
        nonce: &shared.nonce,
        task_repo: shared.task_repo.as_deref(),
    });
    match decision {
        GateDecision::Reject(reject) => {
            let status = reject.status();
            write_reject(&mut stream, status);
            shared.log.record(&head.method, "-", status);
        }
        GateDecision::Forward(plan) => {
            if head.expects_continue() {
                drop(stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n"));
            }
            let method = head.method.clone();
            let route = plan.route.as_str().to_owned();
            let status = shared.handler.forward(
                Forward {
                    head,
                    plan,
                    body_prefix,
                },
                &mut stream,
            );
            shared.log.record(&method, &route, status);
        }
    }
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
