//! Ports of a pool Box's VM: whether anything listens, and one TCP connection carried through
//! the host's agent.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use moor_client::link::broker::is_link_loss;
use moor_client::link::reach::StdConn;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, Forwards, NO_PORTS, OLD_AGENT_PORTS};
use moor_core::seam::proc::Pipes;

use std::sync::Arc;

use super::{failed, is_pool, Crabbox};

/// How the controller reaches the ports of a pool Box's VM: by enrolled machine id, through the
/// runner's links broker.
pub(crate) trait PoolPorts: Send + Sync {
    /// Whether anything listens on `port` of `slug`'s VM on machine `machine_id`.
    fn port_listens(&self, machine_id: i64, slug: &str, port: u16) -> Result<bool, String>;
    /// A blocking stream to `port` of `slug`'s VM on machine `machine_id`.
    fn port_conn(&self, machine_id: i64, slug: &str, port: u16) -> Result<StdConn, String>;
}

impl Crabbox {
    /// Reaches the ports of pool Boxes through `ports`; none refuses every forward.
    pub(crate) fn with_pool_ports(mut self, ports: Option<Arc<dyn PoolPorts>>) -> Self {
        self.pool_ports = ports;
        self
    }

    /// The ports seam and the machine and lease of the pool Box behind `handle`.
    fn reached<'a>(
        &'a self,
        handle: &'a BoxHandle,
    ) -> Result<(&'a dyn PoolPorts, i64, &'a str), DriverError> {
        let placed = self.placed(handle)?;
        let ports = self
            .pool_ports
            .as_deref()
            .ok_or_else(|| failed("this controller reaches no ports of pool Boxes"))?;
        Ok((ports, placed.machine, placed.slug))
    }
}

/// How often a connection checks whether its caller cut it.
const STOP_POLL: Duration = Duration::from_millis(100);

/// What an agent that predates the `port` op says to one.
const OLD_AGENT: &str = "unknown variant `port`";

impl Forwards for Crabbox {
    fn port_listens(&self, handle: &BoxHandle, port: u16) -> Result<bool, DriverError> {
        listens(self, handle, port)
    }

    fn forward_piped(
        &self,
        handle: &BoxHandle,
        port: u16,
        pipes: Pipes<'_>,
    ) -> Result<(), DriverError> {
        forward(self, handle, port, pipes)
    }
}

/// Whether anything listens on `port` of the pool Box's loopback, as its host says.
fn listens(cb: &Crabbox, handle: &BoxHandle, port: u16) -> Result<bool, DriverError> {
    if !is_pool(handle) {
        return Err(DriverError::Unsupported(NO_PORTS));
    }
    let (ports, machine, slug) = cb.reached(handle)?;
    ports.port_listens(machine, slug, port).map_err(refused)
}

/// Carries one connection to `port` of the pool Box's loopback through `pipes`.
fn forward(
    cb: &Crabbox,
    handle: &BoxHandle,
    port: u16,
    pipes: Pipes<'_>,
) -> Result<(), DriverError> {
    if !is_pool(handle) {
        return Err(DriverError::Unsupported(NO_PORTS));
    }
    let (ports, machine, slug) = cb.reached(handle)?;
    let conn = ports.port_conn(machine, slug, port).map_err(refused)?;
    pump(&conn, pipes).map_err(|e| failed(format!("the connection broke: {e}")))
}

/// The error for a host's `why`: an old agent, a lost link, or what the agent refused with.
fn refused(why: String) -> DriverError {
    if why.contains(OLD_AGENT) {
        return DriverError::Unsupported(OLD_AGENT_PORTS);
    }
    if is_link_loss(&why) {
        return DriverError::Transport { stderr_tail: why };
    }
    let said = why
        .split_once(" refused: ")
        .map_or(why.as_str(), |(_, r)| r);
    failed(said.to_owned())
}

/// Copies `pipes.stdin` to the connection and the connection to `pipes.stdout` until either
/// side ends or `pipes.stop` says to cut it.
fn pump(conn: &StdConn, pipes: Pipes<'_>) -> io::Result<()> {
    let Pipes {
        mut stdin,
        stdout,
        stop,
        ..
    } = pipes;
    let mut up = conn.try_clone()?;
    // Detached: it may block on stdin forever after the connection ended.
    thread::spawn(move || {
        let _ = io::copy(&mut stdin, &mut up);
        let _ = up.shutdown(Shutdown::Write);
    });
    let (done, cut) = (AtomicBool::new(false), AtomicBool::new(false));
    thread::scope(|s| {
        let (done, cut) = (&done, &cut);
        s.spawn(move || {
            while !done.load(Ordering::SeqCst) {
                if stop() {
                    cut.store(true, Ordering::SeqCst);
                    let _ = conn.shutdown(Shutdown::Both);
                    return;
                }
                thread::sleep(STOP_POLL);
            }
        });
        let copied = copy_down(conn, stdout);
        done.store(true, Ordering::SeqCst);
        if cut.load(Ordering::SeqCst) {
            return Ok(());
        }
        copied
    })
}

/// Copies what `conn` sends to `out`, flushing each chunk, until it ends.
fn copy_down(mut conn: &StdConn, out: &mut (dyn Write + Send)) -> io::Result<()> {
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = match conn.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.write_all(buf.get(..n).unwrap_or_default())?;
        out.flush()?;
    }
}
