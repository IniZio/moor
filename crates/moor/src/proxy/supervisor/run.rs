//! Startup, heartbeat and shutdown of one supervised proxy.

use std::cell::{Cell, RefCell};
use std::fmt::Arguments;
use std::path::Path;
use std::sync::PoisonError;

use moor_core::seam::driver::TunnelHandle;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId};

use super::beat::Contact;
use super::bind;
use super::listen::{listen, Live};
use super::machine::MachineSlot;
use super::park::{parked_tunnel, starts_parked};
use super::tunnel::OutageSlot;
use super::{Deps, Sink, LEASE};
use crate::proxy::ledger::{BoxView, LedgerError};
use crate::store::{Fenced, Lease, ReadyProc};

/// Why the supervisor stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// A signal asked it to. The row and its ports stay so the Proxy restarts on them.
    Signal,
    /// The Box closed or went away.
    Closed,
    /// Another supervisor took the lease.
    LostLease,
    /// The front socket could not be bound again.
    FrontLost,
    /// A store error made it give up.
    Failed,
}

/// A proxy that is up: lease held, front listening, tunnel open, row ready.
pub(super) struct Running<'d> {
    /// Collaborators.
    pub(super) deps: &'d Deps<'d>,
    /// The supervised Box.
    pub(super) box_id: BoxId,
    /// Lease owner token.
    pub(super) owner: String,
    /// The nono proxy with its capture and log.
    pub(super) live: Live,
    /// The reverse tunnel; dropping it stops the tunnel.
    pub(super) tunnel: RefCell<Box<dyn TunnelHandle>>,
    /// Whether the tunnel is parked on purpose because the Box sleeps.
    pub(super) parked: Cell<bool>,
    /// The Box the tunnel leads into.
    pub(super) handle: BoxHandle,
    /// The Box port the tunnel exposes.
    pub(super) box_port: u16,
    /// The user's machine attached to the Box.
    pub(super) machine: RefCell<MachineSlot>,
    /// Set while the tunnel stays down past the quick restarts.
    pub(super) outage: OutageSlot,
    /// Whether the controller answered the latest heartbeat.
    pub(super) contact: Contact,
}

/// Writes one `moor proxy: ` line to `sink`, ignoring write errors and a poisoned lock.
pub(super) fn say(sink: &Sink, args: Arguments<'_>) {
    drop(writeln!(
        sink.lock().unwrap_or_else(PoisonError::into_inner),
        "moor proxy: {args}"
    ));
}

/// Gives up the lease; a failure only means the lease expires on its own.
fn give_up(deps: &Deps<'_>, owner: &str) {
    if let Err(err) = deps.ledger.release(owner) {
        say(
            &deps.stderr,
            format_args!("cannot release the lease: {err}"),
        );
    }
}

/// Checks the Box, the proxy row, the spec and the machine handle, refusing with exit code 1 when any is missing.
fn preflight(deps: &Deps<'_>, box_id: BoxId) -> Result<(BoxView, BoxHandle, ProxySpec), u8> {
    let sink = &deps.stderr;
    let view = match deps.ledger.view() {
        Ok(Some(view)) if view.open => view,
        Ok(_) => {
            say(
                sink,
                format_args!("Box {} is missing or closed", box_id.get()),
            );
            return Err(1);
        }
        Err(err) => {
            say(sink, format_args!("cannot read the Box: {err}"));
            return Err(1);
        }
    };
    reload(deps, box_id)?;
    let Some(handle) = view.handle.clone() else {
        say(sink, format_args!("Box {} has no machine", box_id.get()));
        return Err(1);
    };
    let spec = match deps.ledger.spec() {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            say(sink, format_args!("no proxy spec for Box {}", box_id.get()));
            return Err(1);
        }
        Err(err) => {
            say(sink, format_args!("cannot read the proxy spec: {err}"));
            return Err(1);
        }
    };
    Ok((view, handle, spec))
}

/// Reads the proxy row's Box port, refusing with exit code 1 when the row is missing.
fn reload(deps: &Deps<'_>, box_id: BoxId) -> Result<u16, u8> {
    match deps.ledger.box_port() {
        Ok(Some(port)) => Ok(port),
        Ok(None) => {
            say(
                &deps.stderr,
                format_args!("Box {} has no proxy row", box_id.get()),
            );
            Err(1)
        }
        Err(err) => {
            say(
                &deps.stderr,
                format_args!("cannot read the proxy row: {err}"),
            );
            Err(1)
        }
    }
}

/// Claims the lease and returns the owner token; `Err(0)` means another supervisor holds it.
fn claim(deps: &Deps<'_>) -> Result<String, u8> {
    let owner = format!(
        "{:08x}{:08x}{:08x}{:08x}",
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32()
    );
    match deps.ledger.claim_lease(&owner, LEASE) {
        Ok(Lease::Claimed) => Ok(owner),
        Ok(Lease::Held { .. }) => Err(0),
        Err(err) => {
            say(&deps.stderr, format_args!("cannot claim the lease: {err}"));
            Err(1)
        }
    }
}

/// Opens the first tunnel into the Box, reporting a failure.
///
/// A Box that had a tunnel before is being served again, often by a supervisor the controller
/// restarted while the machine's link is still down. Its failed open yields the dead stand-in, so
/// the restart path keeps trying; only a new Box's failure refuses the start.
fn open_tunnel(
    deps: &Deps<'_>,
    (handle, box_port, socket): (&BoxHandle, u16, &Path),
    owner: &str,
) -> Option<Box<dyn TunnelHandle>> {
    // A Box that never had a tunnel has no stale listener to clear. A failed read counts as "had one".
    let fresh = matches!(deps.ledger.tunnel_opened(), Ok(false));
    // Set before opening, so a crash mid-open still marks the Box.
    if let Err(err) = deps.ledger.note_tunnel_opened() {
        say(
            &deps.stderr,
            format_args!("cannot record the tunnel open: {err}"),
        );
    }
    match bind::open(deps, owner, handle, (box_port, socket), fresh) {
        Ok(tunnel) => Some(tunnel),
        Err(err) => {
            say(
                &deps.stderr,
                format_args!("cannot open the tunnel into the box: {err}"),
            );
            (!fresh).then(parked_tunnel)
        }
    }
}

/// Claims the lease, starts the proxy and tunnel, and marks the row ready.
///
/// A sleeping machine refuses connections, so a Box that is Asleep or was asked to park starts without a tunnel.
pub(super) fn begin<'d>(deps: &'d Deps<'d>, box_id: BoxId) -> Result<Running<'d>, u8> {
    let sink = &deps.stderr;
    let (view, handle, spec) = preflight(deps, box_id)?;
    let owner = claim(deps)?;
    // Another `moor start` may have re-reserved the row before the claim, so read it again.
    let box_port = match reload(deps, box_id) {
        Ok(found) => found,
        Err(code) => {
            give_up(deps, &owner);
            return Err(code);
        }
    };
    let live = match listen(deps, box_id, &spec) {
        Ok(live) => live,
        Err(msg) => {
            say(sink, format_args!("{msg}"));
            give_up(deps, &owner);
            return Err(1);
        }
    };
    if let Err(code) = renew(deps, &owner) {
        live.stop();
        return Err(code);
    }
    let parked = starts_parked(deps, view.asleep);
    let tunnel = if parked {
        parked_tunnel()
    } else {
        let ends = (&handle, box_port, live.socket());
        let Some(tunnel) = open_tunnel(deps, ends, &owner) else {
            live.stop();
            give_up(deps, &owner);
            return Err(1);
        };
        tunnel
    };
    if let Err(code) = mark(deps, &owner, box_port, &handle) {
        drop(tunnel);
        live.stop();
        return Err(code);
    }
    let run = Running {
        deps,
        box_id,
        owner,
        live,
        tunnel: RefCell::new(tunnel),
        parked: Cell::new(parked),
        handle,
        box_port,
        machine: RefCell::default(),
        outage: OutageSlot::default(),
        contact: Contact::new(),
    };
    if parked {
        // Answers a pending request: Parked is acked, Up reopens the tunnel.
        run.follow_request();
    }
    Ok(run)
}

/// Extends the lease during startup; `Err` carries the exit code, with the lease given up unless it was lost.
fn renew(deps: &Deps<'_>, owner: &str) -> Result<(), u8> {
    match deps.ledger.heartbeat(owner) {
        Ok(true) => Ok(()),
        Ok(false) => {
            say(
                &deps.stderr,
                format_args!("another supervisor took the lease; stopping"),
            );
            Err(1)
        }
        Err(err) => {
            say(&deps.stderr, format_args!("heartbeat failed: {err}"));
            give_up(deps, owner);
            Err(1)
        }
    }
}

/// Marks the row ready; `Err` carries the exit code, with the lease given up unless it was lost.
fn mark(deps: &Deps<'_>, owner: &str, box_port: u16, handle: &BoxHandle) -> Result<(), u8> {
    renew(deps, owner)?;
    match deps.ledger.mark_ready(
        owner,
        box_port,
        ReadyProc {
            pid: std::process::id(),
            handle: handle.as_str(),
            boot_id: deps.boot_id.as_deref(),
        },
    ) {
        Ok(Fenced::Applied) => Ok(()),
        Ok(Fenced::LostLease) => {
            say(
                &deps.stderr,
                format_args!("another supervisor took the lease; stopping"),
            );
            Err(1)
        }
        Err(err) => {
            say(
                &deps.stderr,
                format_args!("cannot mark the proxy ready: {err}"),
            );
            give_up(deps, owner);
            Err(1)
        }
    }
}

impl Running<'_> {
    /// Heartbeats and checks the Box, the front socket and the tunnel; `None` means keep running.
    pub(super) fn tick(&self) -> Option<Stop> {
        if let stop @ Some(_) = self.beat() {
            return stop;
        }
        self.live.drain(self.deps);
        let dropped = self.deps.ledger.dropped();
        if dropped > 0 {
            say(
                &self.deps.stderr,
                format_args!("dropped {dropped} audit events the controller could not take"),
            );
        }
        if !self.contact.reached() {
            return None;
        }
        self.deps.ledger.flush();
        match self.deps.ledger.view() {
            Ok(Some(view)) if view.open => {}
            Ok(_) => return Some(Stop::Closed),
            Err(LedgerError::Unavailable(_)) => return None,
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot read the Box: {err}"),
                );
                return Some(Stop::Failed);
            }
        }
        self.follow_request();
        self.tend_machine();
        if let Err(err) = self.live.rebind() {
            say(
                &self.deps.stderr,
                format_args!("cannot bind the proxy front again: {err}"),
            );
            return Some(Stop::FrontLost);
        }
        if self.parked.get() || self.tunnel.borrow().alive() {
            self.outage.set(None);
            return None;
        }
        self.revive()
    }

    /// Stops the tunnel and listener, tidies the row for `stop` and returns the exit code.
    pub(super) fn finish(self, stop: Stop) -> u8 {
        if stop == Stop::Closed {
            self.let_go_machine();
        }
        let Self {
            deps,
            box_id,
            owner,
            live,
            tunnel,
            ..
        } = self;
        live.drain(deps);
        live.stop();
        drop(tunnel);
        if stop != Stop::LostLease {
            give_up(deps, &owner);
        }
        if stop == Stop::Closed {
            if let Err(err) = deps.ledger.remove_proxy() {
                say(
                    &deps.stderr,
                    format_args!("cannot remove the proxy row: {err}"),
                );
            }
            Live::forget(deps, box_id);
        }
        u8::from(!matches!(stop, Stop::Closed | Stop::Signal))
    }

    /// The lease owner token.
    #[cfg(test)]
    pub(super) fn owner(&self) -> &str {
        &self.owner
    }

    /// The socket of the CONNECT-only front the Box reaches.
    #[cfg(test)]
    pub(super) fn socket(&self) -> &Path {
        self.live.socket()
    }
}
