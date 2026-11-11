//! Port forwards: a person's computer reaches a port on a Box's loopback.
//!
//! `forward-open` wakes a sleeping Box, checks that something listens and records the forward.
//! Each browser connection is a `forward` request that carries its bytes; a connection never
//! wakes a Box, so a tab that keeps retrying cannot hold one awake. `forward-close` records
//! what the forward carried.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::AccessEvent;
use moor_core::name::BoxId;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::{DriverError, NO_PORTS, OLD_AGENT_PORTS};
use moor_core::seam::proc::Pipes;
use moor_core::view::{ForwardView, ForwardedView};
use moor_core::{ErrorKind, ForwardCloseReq, ForwardOpenReq, ForwardReq};

use crate::cx::owners::Cmd;
use crate::cx::traffic::Traffic;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provisioning::dead_provisioning;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::wake::wake_and_revive;
use crate::ops::run::ready;
use crate::store::{BoxRow, Store};

/// How long a connection may carry no byte either way before it is cut.
const IDLE_CUT: Duration = Duration::from_mins(30);

/// Wakes the Box if it sleeps, checks that something listens on its `port`, and records the
/// forward.
///
/// # Errors
/// [`CommandError`] when the Box is closed, cannot be woken, is not on a node that forwards
/// ports, or nothing listens; the refusal is also recorded on the Box.
pub(crate) fn open(cx: &Cx, req: &ForwardOpenReq) -> Result<ForwardView, CommandError> {
    let store = cx.store()?;
    let first = open_box(store, &req.target)?;
    let _turn = cx.turn(first.id, Cmd::Exec)?;
    let opened = open_under_turn(cx, store, req, &first);
    if let Err(e) = &opened {
        let event = AccessEvent::PortForwardRefused {
            port: req.port,
            reason: e.error.to_string(),
        };
        let _ = store.apply(Decision::new(first.id).event(ExtraEvent::Access(event)));
    }
    opened
}

/// [`open`] with the Box's turn held: wakes it, checks the port and records the forward.
fn open_under_turn(
    cx: &Cx,
    store: &Store,
    req: &ForwardOpenReq,
    first: &BoxRow,
) -> Result<ForwardView, CommandError> {
    dead_provisioning(cx, first)?;
    let mut row = store.box_row(first.id)?.unwrap_or_else(|| first.clone());
    if row.asleep() && wake_and_revive(cx, store, &row)? {
        row = store.box_row(first.id)?.unwrap_or(row);
    }
    let target = ready::target(store, &row)?;
    match cx.driver().port_listens(&target.handle, req.port) {
        Ok(true) => {}
        Ok(false) => {
            let why = format!(
                "nothing listens on port {} in {}",
                req.port,
                row.name.as_str()
            );
            return Err(CommandError::new(ErrorKind::Failed, why));
        }
        Err(e) => return Err(driver_failure(&row, req.port, &e)),
    }
    let opened = AccessEvent::PortForwardOpened {
        port: req.port,
        local_port: req.local_port,
    };
    store.apply(Decision::new(row.id).event(ExtraEvent::Access(opened)))?;
    Ok(ForwardView { port: req.port })
}

/// Carries one TCP connection to `port` of the Box's loopback through `pipes`.
///
/// # Errors
/// [`CommandError`] when the Box is asleep or closed, or the connection cannot be made.
pub(crate) fn connect(
    cx: &Cx,
    req: &ForwardReq,
    pipes: Pipes<'_>,
) -> Result<ForwardedView, CommandError> {
    let store = cx.store()?;
    let row = open_box(store, &req.target)?;
    let name = row.name.as_str();
    if row.asleep() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("{name} is asleep; open the forward again to wake it"),
        ));
    }
    let target = ready::target(store, &row)?;
    let tally = Arc::new(Tally::new(row.id, Arc::clone(cx.traffic()), cx.clock_arc()));
    let Pipes {
        stdin,
        stdout,
        stderr,
        stop,
    } = pipes;
    let mut out = Tap {
        inner: stdout,
        tally: &tally,
        counts: &tally.bytes_out,
    };
    let idle = || tally.idle_for() >= IDLE_CUT;
    let stop = || stop() || idle();
    let pipes = Pipes {
        stdin: Box::new(Feed {
            inner: stdin,
            tally: Arc::clone(&tally),
        }),
        stdout: &mut out,
        stderr,
        stop: &stop,
    };
    cx.driver()
        .forward_piped(&target.handle, req.port, pipes)
        .map_err(|e| driver_failure(&row, req.port, &e))?;
    Ok(ForwardedView {
        bytes_in: tally.bytes_in.load(Ordering::SeqCst),
        bytes_out: tally.bytes_out.load(Ordering::SeqCst),
    })
}

/// Records that the forward ended with the counts the client kept. A Box that is gone since
/// leaves nothing to record on.
///
/// # Errors
/// [`CommandError`] on a store failure.
pub(crate) fn close(cx: &Cx, req: &ForwardCloseReq) -> Result<ForwardView, CommandError> {
    let store = cx.store()?;
    if let Some(row) = target::resolve(store, &req.target, Rule::Latest)? {
        let _turn = cx.turn(row.id, Cmd::Exec)?;
        let closed = ExtraEvent::Access(AccessEvent::PortForwardClosed {
            port: req.port,
            connections: req.connections,
            bytes_in: req.bytes_in,
            bytes_out: req.bytes_out,
        });
        store.apply(Decision::new(row.id).event(closed))?;
    }
    Ok(ForwardView { port: req.port })
}

/// The open Box `target` means, or the refusal that it is closed.
fn open_box(store: &Store, target: &moor_core::BoxTarget) -> Result<BoxRow, CommandError> {
    target::resolve(store, target, Rule::Open)?.ok_or_else(|| {
        CommandError::new(
            ErrorKind::Refused,
            format!("{target} is closed; the forward ended"),
        )
    })
}

/// What a driver failure of a forward says to the person.
fn driver_failure(row: &BoxRow, port: u16, e: &DriverError) -> CommandError {
    let name = row.name.as_str();
    let provider = row.hands.as_ref().map_or("", |h| h.provider.as_str());
    let host = provider.split_once('@').map(|(_, host)| host);
    let (kind, why) = match e {
        DriverError::Unsupported(what) if *what == NO_PORTS => (
            ErrorKind::Refused,
            format!("port forward needs a Box on a node; {name} runs on {provider}"),
        ),
        DriverError::Unsupported(what) if *what == OLD_AGENT_PORTS => (
            ErrorKind::Failed,
            format!(
                "the machine agent on {} does not forward ports yet; it updates itself while linked",
                host.unwrap_or(provider)
            ),
        ),
        DriverError::Transport { .. } => (
            ErrorKind::Failed,
            format!(
                "{name}'s node {} is not linked to moor",
                host.unwrap_or(provider)
            ),
        ),
        DriverError::Failed { summary } if summary.starts_with("already ") => {
            (ErrorKind::Refused, format!("{name} {summary}"))
        }
        DriverError::Failed { summary } => (
            ErrorKind::Failed,
            format!("cannot forward port {port} in {name}: {summary}"),
        ),
        other => (
            ErrorKind::Failed,
            format!("cannot forward port {port} in {name}: {other}"),
        ),
    };
    CommandError::new(kind, why)
}

/// What one connection has carried, and when it last carried a byte.
struct Tally {
    /// The Box the connection is into.
    id: BoxId,
    /// Where the Box's recent traffic is noted.
    traffic: Arc<Traffic>,
    /// The time source.
    clock: Arc<dyn Clock>,
    /// Bytes from the person into the Box.
    bytes_in: AtomicU64,
    /// Bytes from the Box back to the person.
    bytes_out: AtomicU64,
    /// When a byte last moved either way.
    last: Mutex<Instant>,
}

impl Tally {
    /// A tally for a connection that starts now.
    fn new(id: BoxId, traffic: Arc<Traffic>, clock: Arc<dyn Clock>) -> Self {
        let last = Mutex::new(clock.instant());
        Self {
            id,
            traffic,
            clock,
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            last,
        }
    }

    /// Notes that `n` bytes moved, adding them to `side`.
    fn moved(&self, side: &AtomicU64, n: usize) {
        if n == 0 {
            return;
        }
        side.fetch_add(n as u64, Ordering::SeqCst);
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = self.clock.instant();
        self.traffic.touch(self.id, self.clock.now());
    }

    /// How long no byte has moved.
    fn idle_for(&self) -> Duration {
        let last = *self.last.lock().unwrap_or_else(PoisonError::into_inner);
        self.clock.instant().saturating_duration_since(last)
    }
}

/// The person's bytes going into the Box, counted.
struct Feed {
    /// The person's side.
    inner: Box<dyn Read + Send>,
    /// What the connection has carried.
    tally: Arc<Tally>,
}

impl Read for Feed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.tally.moved(&self.tally.bytes_in, n);
        Ok(n)
    }
}

/// The Box's bytes going back to the person, counted.
struct Tap<'a> {
    /// The person's side.
    inner: &'a mut (dyn Write + Send),
    /// What the connection has carried.
    tally: &'a Tally,
    /// The count this side adds to.
    counts: &'a AtomicU64,
}

impl Write for Tap<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.tally.moved(self.counts, n);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
