//! Connection limits: an overall request-read deadline and a concurrent-connection cap.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Bounds on how long and how many clients the server serves at once.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Total time a client has to send its whole request, head and body.
    pub read_deadline: Duration,
    /// Connections served at once; more are answered 503.
    pub max_connections: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            read_deadline: Duration::from_secs(10),
            max_connections: 64,
        }
    }
}

/// Current instant; socket timeouts run on real time, so this does not use the clock seam.
#[allow(
    clippy::disallowed_methods,
    reason = "socket timeouts are real time and cannot be driven by a fake clock"
)]
pub(super) fn now() -> Instant {
    Instant::now()
}

/// Reads from a stream until a fixed deadline, then fails with `TimedOut`.
pub(super) struct Deadline<'a> {
    /// The connection being read.
    stream: &'a TcpStream,
    /// Instant after which reads fail.
    end: Instant,
}

impl<'a> Deadline<'a> {
    /// Starts a deadline of `budget` from now.
    pub(super) fn new(stream: &'a TcpStream, budget: Duration) -> Self {
        Self {
            stream,
            end: now() + budget,
        }
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_duration_since(now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(out).map_err(|e| match e.kind() {
            io::ErrorKind::WouldBlock => io::ErrorKind::TimedOut.into(),
            _ => e,
        })
    }
}

/// Counts connections being served.
#[derive(Debug, Clone, Default)]
pub(super) struct Gate(Arc<AtomicUsize>);

impl Gate {
    /// Takes a slot unless `max` are already held.
    pub(super) fn try_acquire(&self, max: usize) -> Option<Guard> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then_some(n.saturating_add(1))
            })
            .ok()?;
        Some(Guard(Arc::clone(&self.0)))
    }

    /// Takes a slot for `stream`, or answers it 503 when `max` are held.
    pub(super) fn admit(&self, stream: &mut TcpStream, max: usize) -> Option<Guard> {
        let slot = self.try_acquire(max);
        if slot.is_none() {
            reject(stream);
        }
        slot
    }
}

/// A held connection slot, released on drop.
#[derive(Debug)]
pub(super) struct Guard(Arc<AtomicUsize>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers 408 on a connection whose request did not arrive in time.
pub(super) fn timed_out(stream: &mut TcpStream) -> io::Result<()> {
    super::respond(
        stream,
        "408 Request Timeout",
        "text/plain",
        &[],
        "request timeout",
    )
}

/// Answers 503 on `stream` without blocking the accept loop, then closes it.
fn reject(stream: &mut TcpStream) {
    const BODY: &str = "server busy";
    let reply = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\nRetry-After: 1\r\n\r\n{BODY}",
        BODY.len()
    );
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.shutdown(Shutdown::Write);
    // Discard what the client already sent so closing does not reset the reply.
    if stream.set_nonblocking(true).is_ok() {
        let _ = stream.read(&mut [0u8; 1024]);
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the deadline reader and the connection gate.
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn gate_stops_at_max_and_frees_on_drop() {
        let gate = Gate::default();
        let a = gate.try_acquire(2);
        let b = gate.try_acquire(2);
        assert!(a.is_some() && b.is_some());
        assert!(gate.try_acquire(2).is_none());
        drop(a);
        assert!(gate.try_acquire(2).is_some());
        assert_eq!(gate.0.load(Ordering::Acquire), 1);
    }

    #[test]
    fn expired_deadline_fails_without_reading() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut reader = Deadline::new(&client, Duration::ZERO);
        let err = reader.read(&mut [0u8; 4]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn silent_peer_times_out_at_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let start = now();
        let mut reader = Deadline::new(&client, Duration::from_millis(100));
        let err = reader.read(&mut [0u8; 4]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
