//! Startup, heartbeat and shutdown of one supervised proxy.

use std::cell::RefCell;
use std::fmt::Arguments;
use std::path::PathBuf;
use std::sync::PoisonError;

use moor_core::domain::task::TaskState;
use moor_core::seam::driver::TunnelHandle;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, Secret, TaskId};

use super::listen::{listen, Live};
use super::{Deps, Sink, LEASE};
use crate::proxy::detached::read_spec;
use crate::store::{Fenced, Lease, ProxyPorts, ReadyProc, TaskRow};

/// Why the supervisor stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// A signal asked it to.
    Signal,
    /// The Task closed or went away.
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
    /// The supervised Task.
    pub(super) task: TaskId,
    /// Lease owner token.
    pub(super) owner: String,
    /// The nono proxy with its capture and log.
    pub(super) live: Live,
    /// The reverse tunnel; dropping it stops the tunnel.
    pub(super) tunnel: RefCell<Box<dyn TunnelHandle>>,
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

/// Whether no more work can happen on a Task in `state`.
const fn terminal(state: TaskState) -> bool {
    matches!(state, TaskState::Closed | TaskState::Rejected)
}

/// Gives up the lease; a failure only means the lease expires on its own.
fn give_up(deps: &Deps<'_>, task: TaskId, owner: &str) {
    if let Err(err) = deps.store.release(task, owner) {
        say(
            &deps.stderr,
            format_args!("cannot release the lease: {err}"),
        );
    }
}

/// Checks the Task, the proxy row, the spec and the Box handle, refusing with exit code 1 when any is missing.
fn preflight(deps: &Deps<'_>, task: TaskId) -> Result<(TaskRow, BoxHandle, ProxySpec), u8> {
    let sink = &deps.stderr;
    let task_row = match deps.store.task(task) {
        Ok(Some(row)) if !terminal(row.state) => row,
        Ok(_) => {
            say(
                sink,
                format_args!("task {} is missing or closed", task.get()),
            );
            return Err(1);
        }
        Err(err) => {
            say(sink, format_args!("cannot read the task: {err}"));
            return Err(1);
        }
    };
    reload(deps, task)?;
    let Some(handle) = task_row.boxed.as_ref().and_then(|b| b.handle.clone()) else {
        say(sink, format_args!("task {} has no box", task.get()));
        return Err(1);
    };
    let Ok(spec) = read_spec(deps.state_dir, task) else {
        say(sink, format_args!("no proxy spec for task {}", task.get()));
        return Err(1);
    };
    Ok((task_row, handle, spec))
}

/// Reads the proxy row's nonce and Box port, refusing with exit code 1 when the row is missing.
fn reload(deps: &Deps<'_>, task: TaskId) -> Result<(Secret, u16), u8> {
    match deps.store.proxy(task) {
        Ok(Some(row)) => Ok((row.nonce, row.box_port)),
        Ok(None) => {
            say(
                &deps.stderr,
                format_args!("task {} has no proxy row", task.get()),
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
fn claim(deps: &Deps<'_>, task: TaskId) -> Result<String, u8> {
    let owner = format!(
        "{:08x}{:08x}{:08x}{:08x}",
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32(),
        deps.rng.u32()
    );
    match deps.store.claim_lease(task, &owner, LEASE) {
        Ok(Lease::Claimed) => Ok(owner),
        Ok(Lease::Held { .. }) => Err(0),
        Err(err) => {
            say(&deps.stderr, format_args!("cannot claim the lease: {err}"));
            Err(1)
        }
    }
}

/// Claims the lease, starts the proxy and tunnel, and marks the row ready.
pub(super) fn begin<'d>(deps: &'d Deps<'d>, task: TaskId) -> Result<Running<'d>, u8> {
    let sink = &deps.stderr;
    let (task_row, handle, spec) = preflight(deps, task)?;
    let owner = claim(deps, task)?;
    // Another `moor start` may have re-reserved the row before the claim, so read it again.
    let (nonce, box_port) = match reload(deps, task) {
        Ok(found) => found,
        Err(code) => {
            give_up(deps, task, &owner);
            return Err(code);
        }
    };
    let worktree = PathBuf::from(task_row.worktree.as_deref().unwrap_or(&task_row.repo));
    let live = match listen(deps, task, &nonce, &spec) {
        Ok(live) => live,
        Err(msg) => {
            say(sink, format_args!("{msg}"));
            give_up(deps, task, &owner);
            return Err(1);
        }
    };
    if let Err(code) = renew(deps, task, &owner) {
        live.proxy.stop();
        return Err(code);
    }
    let tunnel = match deps
        .driver
        .reverse_tunnel(&handle, box_port, live.proxy.port(), &worktree)
    {
        Ok(tunnel) => tunnel,
        Err(err) => {
            say(
                sink,
                format_args!("cannot open the tunnel into the box: {err}"),
            );
            live.proxy.stop();
            give_up(deps, task, &owner);
            return Err(1);
        }
    };
    let ports = ProxyPorts {
        box_port,
        host_port: live.proxy.port(),
    };
    if let Err(code) = mark(deps, task, &owner, ports, &handle) {
        drop(tunnel);
        live.proxy.stop();
        return Err(code);
    }
    Ok(Running {
        deps,
        task,
        owner,
        live,
        tunnel: RefCell::new(tunnel),
        handle,
        box_port,
        worktree,
    })
}

/// Extends the lease during startup; `Err` carries the exit code, with the lease given up unless it was lost.
fn renew(deps: &Deps<'_>, task: TaskId, owner: &str) -> Result<(), u8> {
    match deps.store.heartbeat(task, owner) {
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
            give_up(deps, task, owner);
            Err(1)
        }
    }
}

/// Marks the row ready; `Err` carries the exit code, with the lease given up unless it was lost.
fn mark(
    deps: &Deps<'_>,
    task: TaskId,
    owner: &str,
    ports: ProxyPorts,
    handle: &BoxHandle,
) -> Result<(), u8> {
    renew(deps, task, owner)?;
    match deps.store.mark_ready(
        task,
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
            give_up(deps, task, owner);
            Err(1)
        }
    }
}

impl Running<'_> {
    /// Extends the lease; `None` means it is still held.
    pub(super) fn beat(&self) -> Option<Stop> {
        match self.deps.store.heartbeat(self.task, &self.owner) {
            Ok(true) => None,
            Ok(false) => Some(Stop::LostLease),
            Err(err) => {
                say(&self.deps.stderr, format_args!("heartbeat failed: {err}"));
                Some(Stop::Failed)
            }
        }
    }

    /// Heartbeats and checks the Task and tunnel; `None` means keep running.
    pub(super) fn tick(&self) -> Option<Stop> {
        if let stop @ Some(_) = self.beat() {
            return stop;
        }
        self.live.drain(self.deps, self.task);
        match self.deps.store.task(self.task) {
            Ok(Some(row)) if !terminal(row.state) => {}
            Ok(_) => return Some(Stop::Closed),
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot read the task: {err}"),
                );
                return Some(Stop::Failed);
            }
        }
        if self.tunnel.borrow().alive() {
            return None;
        }
        self.revive()
    }

    /// Stops the tunnel and listener, tidies the row for `stop` and returns the exit code.
    pub(super) fn finish(self, stop: Stop) -> u8 {
        let Self {
            deps,
            task,
            owner,
            live,
            tunnel,
            ..
        } = self;
        live.drain(deps, task);
        live.proxy.stop();
        drop(tunnel);
        if stop != Stop::LostLease {
            give_up(deps, task, &owner);
        }
        if matches!(stop, Stop::Closed | Stop::Signal) {
            if let Err(err) = deps.store.remove_proxy(task) {
                say(
                    &deps.stderr,
                    format_args!("cannot remove the proxy row: {err}"),
                );
            }
        }
        if stop == Stop::Closed {
            Live::forget(deps, task);
        }
        u8::from(!matches!(stop, Stop::Closed | Stop::Signal))
    }

    /// The lease owner token.
    #[cfg(test)]
    pub(super) fn owner(&self) -> &str {
        &self.owner
    }

    /// The port the proxy is bound to.
    #[cfg(test)]
    pub(super) const fn host_port(&self) -> u16 {
        self.live.proxy.port()
    }
}
