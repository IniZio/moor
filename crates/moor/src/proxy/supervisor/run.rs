//! Startup, heartbeat and shutdown of one supervised proxy.

use std::cell::{Cell, RefCell};
use std::fmt::Arguments;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::seam::driver::TunnelHandle;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId, Secret};

use super::bind;
use super::listen::{listen, Live};
use super::park::{parked_tunnel, starts_parked};
use super::{Deps, Sink, LEASE};
use crate::proxy::detached::read_spec;
use crate::store::{BoxRow, Fenced, Lease, ProxyPorts, ReadyProc};

/// Why the supervisor stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// A signal asked it to.
    Signal,
    /// The Box closed or went away.
    Closed,
    /// Another supervisor took the lease.
    LostLease,
    /// The tunnel into the Box exited.
    TunnelDead,
    /// A store error made it give up.
    Failed,
}

/// A proxy that is up: lease held, proxy listening, tunnel open, row ready.
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
    /// Host working directory for the tunnel process.
    pub(super) worktree: PathBuf,
}

/// Writes one `moor proxy: ` line to `sink`, ignoring write errors and a poisoned lock.
pub(super) fn say(sink: &Sink, args: Arguments<'_>) {
    drop(writeln!(
        sink.lock().unwrap_or_else(PoisonError::into_inner),
        "moor proxy: {args}"
    ));
}

/// Whether no more work can happen on a Box in `lifecycle`.
const fn terminal(lifecycle: BoxLifecycle) -> bool {
    matches!(lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
}

/// Gives up the lease; a failure only means the lease expires on its own.
fn give_up(deps: &Deps<'_>, box_id: BoxId, owner: &str) {
    if let Err(err) = deps.store.release(box_id, owner) {
        say(
            &deps.stderr,
            format_args!("cannot release the lease: {err}"),
        );
    }
}

/// Checks the Box, the proxy row, the spec and the machine handle, refusing with exit code 1 when any is missing.
fn preflight(deps: &Deps<'_>, box_id: BoxId) -> Result<(BoxRow, BoxHandle, ProxySpec), u8> {
    let sink = &deps.stderr;
    let box_row = match deps.store.box_row(box_id) {
        Ok(Some(row)) if !terminal(row.lifecycle) => row,
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
    let Some(handle) = box_row.boxed.as_ref().and_then(|b| b.handle.clone()) else {
        say(sink, format_args!("Box {} has no machine", box_id.get()));
        return Err(1);
    };
    let Ok(spec) = read_spec(deps.state_dir, box_id) else {
        say(sink, format_args!("no proxy spec for Box {}", box_id.get()));
        return Err(1);
    };
    Ok((box_row, handle, spec))
}

/// Reads the proxy row's nonce and Box port, refusing with exit code 1 when the row is missing.
fn reload(deps: &Deps<'_>, box_id: BoxId) -> Result<(Secret, u16), u8> {
    match deps.store.proxy(box_id) {
        Ok(Some(row)) => Ok((row.nonce, row.box_port)),
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
fn claim(deps: &Deps<'_>, box_id: BoxId) -> Result<String, u8> {
    let owner = format!(
        "{:08x}{:08x}{:08x}{:08x}",
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32()
    );
    match deps.store.claim_lease(box_id, &owner, LEASE) {
        Ok(Lease::Claimed) => Ok(owner),
        Ok(Lease::Held { .. }) => Err(0),
        Err(err) => {
            say(&deps.stderr, format_args!("cannot claim the lease: {err}"));
            Err(1)
        }
    }
}

/// Whether the Box is stored as Asleep.
fn asleep(row: &BoxRow) -> bool {
    row.boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep)
}

/// Opens the first tunnel into the Box, reporting a failure.
fn open_tunnel(
    deps: &Deps<'_>,
    (handle, box_port, host_port, worktree): (&BoxHandle, u16, u16, &Path),
    box_id: BoxId,
) -> Option<Box<dyn TunnelHandle>> {
    // A Box that never had a tunnel has no stale listener to clear. A failed read counts as "had one".
    let fresh = matches!(deps.store.tunnel_opened(box_id), Ok(false));
    // Set before opening, so a crash mid-open still marks the Box.
    if let Err(err) = deps.store.note_tunnel_opened(box_id) {
        say(
            &deps.stderr,
            format_args!("cannot record the tunnel open: {err}"),
        );
    }
    match bind::open(deps, handle, box_port, host_port, worktree, fresh) {
        Ok(tunnel) => Some(tunnel),
        Err(err) => {
            say(
                &deps.stderr,
                format_args!("cannot open the tunnel into the box: {err}"),
            );
            None
        }
    }
}

/// Claims the lease, starts the proxy and tunnel, and marks the row ready.
///
/// A sleeping machine refuses connections, so a Box that is Asleep or was asked to park starts without a tunnel.
pub(super) fn begin<'d>(deps: &'d Deps<'d>, box_id: BoxId) -> Result<Running<'d>, u8> {
    let sink = &deps.stderr;
    let (box_row, handle, spec) = preflight(deps, box_id)?;
    let owner = claim(deps, box_id)?;
    // Another `moor start` may have re-reserved the row before the claim, so read it again.
    let (nonce, box_port) = match reload(deps, box_id) {
        Ok(found) => found,
        Err(code) => {
            give_up(deps, box_id, &owner);
            return Err(code);
        }
    };
    let worktree = PathBuf::from(box_row.worktree.as_deref().unwrap_or(&box_row.repo));
    let live = match listen(deps, box_id, &nonce, &spec) {
        Ok(live) => live,
        Err(msg) => {
            say(sink, format_args!("{msg}"));
            give_up(deps, box_id, &owner);
            return Err(1);
        }
    };
    if let Err(code) = renew(deps, box_id, &owner) {
        live.stop();
        return Err(code);
    }
    let parked = starts_parked(deps, box_id, asleep(&box_row));
    let tunnel = if parked {
        parked_tunnel()
    } else {
        let ends = (&handle, box_port, live.port(), worktree.as_path());
        let Some(tunnel) = open_tunnel(deps, ends, box_id) else {
            live.stop();
            give_up(deps, box_id, &owner);
            return Err(1);
        };
        tunnel
    };
    let ports = ProxyPorts {
        box_port,
        host_port: live.port(),
    };
    if let Err(code) = mark(deps, box_id, &owner, ports, &handle) {
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
        worktree,
    };
    if parked {
        // Answers a pending request: Parked is acked, Up reopens the tunnel.
        run.follow_request();
    }
    Ok(run)
}

/// Extends the lease during startup; `Err` carries the exit code, with the lease given up unless it was lost.
fn renew(deps: &Deps<'_>, box_id: BoxId, owner: &str) -> Result<(), u8> {
    match deps.store.heartbeat(box_id, owner) {
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
            give_up(deps, box_id, owner);
            Err(1)
        }
    }
}

/// Marks the row ready; `Err` carries the exit code, with the lease given up unless it was lost.
fn mark(
    deps: &Deps<'_>,
    box_id: BoxId,
    owner: &str,
    ports: ProxyPorts,
    handle: &BoxHandle,
) -> Result<(), u8> {
    renew(deps, box_id, owner)?;
    match deps.store.mark_ready(
        box_id,
        owner,
        ports,
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
            give_up(deps, box_id, owner);
            Err(1)
        }
    }
}

impl Running<'_> {
    /// Extends the lease; `None` means it is still held.
    pub(super) fn beat(&self) -> Option<Stop> {
        match self.deps.store.heartbeat(self.box_id, &self.owner) {
            Ok(true) => None,
            Ok(false) => Some(Stop::LostLease),
            Err(err) => {
                say(&self.deps.stderr, format_args!("heartbeat failed: {err}"));
                Some(Stop::Failed)
            }
        }
    }

    /// Heartbeats and checks the Box and tunnel; `None` means keep running.
    pub(super) fn tick(&self) -> Option<Stop> {
        if let stop @ Some(_) = self.beat() {
            return stop;
        }
        self.live.drain(self.deps, self.box_id);
        match self.deps.store.box_row(self.box_id) {
            Ok(Some(row)) if !terminal(row.lifecycle) => {}
            Ok(_) => return Some(Stop::Closed),
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot read the Box: {err}"),
                );
                return Some(Stop::Failed);
            }
        }
        self.follow_request();
        if self.parked.get() || self.tunnel.borrow().alive() {
            return None;
        }
        self.revive()
    }

    /// Stops the tunnel and listener, tidies the row for `stop` and returns the exit code.
    pub(super) fn finish(self, stop: Stop) -> u8 {
        let Self {
            deps,
            box_id,
            owner,
            live,
            tunnel,
            ..
        } = self;
        live.drain(deps, box_id);
        live.stop();
        drop(tunnel);
        if stop != Stop::LostLease {
            give_up(deps, box_id, &owner);
        }
        if matches!(stop, Stop::Closed | Stop::Signal) {
            if let Err(err) = deps.store.remove_proxy(box_id) {
                say(
                    &deps.stderr,
                    format_args!("cannot remove the proxy row: {err}"),
                );
            }
        }
        if stop == Stop::Closed {
            Live::forget(deps, box_id);
        }
        u8::from(!matches!(stop, Stop::Closed | Stop::Signal))
    }

    /// The lease owner token.
    #[cfg(test)]
    pub(super) fn owner(&self) -> &str {
        &self.owner
    }

    /// The port of the CONNECT-only front the Box reaches.
    #[cfg(test)]
    pub(super) const fn host_port(&self) -> u16 {
        self.live.port()
    }
}
