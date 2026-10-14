//! Startup, heartbeat and shutdown of one supervised proxy.

use std::fmt::Arguments;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use moor_core::domain::task::TaskState;
use moor_core::proxy_gate::{RouteRow, RouteTable};
use moor_core::seam::driver::TunnelHandle;
use moor_core::{BoxHandle, Secret, TaskId};

use super::{creds, Deps, Sink, LEASE, MAX_CONNS};
use crate::proxy::relay::Relay;
use crate::proxy::resolver::Resolver;
use crate::proxy::server::{body_cap, ProxyOpts, HEAD_BUDGET};
use crate::proxy::{Proxy, ProxyLog};
use crate::store::{Fenced, Lease, ProxyPorts, TaskRow};

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

/// A proxy that is up: lease held, listener bound, tunnel open, row ready.
pub(super) struct Running<'d> {
    /// Collaborators.
    deps: &'d Deps<'d>,
    /// The supervised Task.
    task: TaskId,
    /// Lease owner token.
    owner: String,
    /// The loopback listener.
    proxy: Proxy,
    /// The reverse tunnel; dropping it stops the tunnel.
    tunnel: Box<dyn TunnelHandle>,
}

/// Writes one `moor proxy: ` line to `sink`, ignoring write errors and a poisoned lock.
fn say(sink: &Sink, args: Arguments<'_>) {
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

/// Reads the Task, the proxy row's nonce and Box port, and the Box handle, refusing with exit code 1 when any is missing.
fn preflight(deps: &Deps<'_>, task: TaskId) -> Result<(TaskRow, Secret, u16, BoxHandle), u8> {
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
    let row = match deps.store.proxy(task) {
        Ok(Some(row)) => row,
        Ok(None) => {
            say(sink, format_args!("task {} has no proxy row", task.get()));
            return Err(1);
        }
        Err(err) => {
            say(sink, format_args!("cannot read the proxy row: {err}"));
            return Err(1);
        }
    };
    let Some(handle) = task_row.boxed.as_ref().and_then(|b| b.handle.clone()) else {
        say(sink, format_args!("task {} has no box", task.get()));
        return Err(1);
    };
    Ok((task_row, row.nonce, row.box_port, handle))
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

/// Opens the proxy log and binds the listener; the caller still holds the lease on failure.
fn listen(deps: &Deps<'_>, task: TaskId, nonce: Secret, worktree: &Path) -> Result<Proxy, String> {
    let repo = (deps.origin_of)(worktree).and_then(|origin| deps.forge.repo_of(&origin));
    let mut rows = vec![RouteRow::model()];
    if repo.is_some() {
        rows.extend(deps.forge.routes());
    }
    let task_repo = repo.as_ref().map(ToString::to_string);
    let resolver = Resolver::new(
        Arc::clone(&deps.model),
        Arc::clone(&deps.forge),
        repo,
        Arc::clone(&deps.clock),
    );
    let creds = Arc::new(creds::Logged::new(resolver, Arc::clone(&deps.stderr)));
    let relay = Relay::new(
        deps.upstreams.clone(),
        creds,
        Arc::clone(&deps.clock),
        body_cap,
    );
    let log = ProxyLog::open(deps.state_dir, task, Arc::clone(&deps.clock))
        .map_err(|err| format!("cannot open the proxy log: {err}"))?;
    let opts = ProxyOpts {
        max_conns: MAX_CONNS,
        task_repo,
        clock: Arc::clone(&deps.clock),
        head_budget: HEAD_BUDGET,
        body_cap,
    };
    Proxy::start(RouteTable::new(rows), nonce, log, Arc::new(relay), opts)
        .map_err(|err| format!("cannot start the listener: {err}"))
}

/// Claims the lease, starts the listener and tunnel, and marks the row ready.
pub(super) fn begin<'d>(deps: &'d Deps<'d>, task: TaskId) -> Result<Running<'d>, u8> {
    let sink = &deps.stderr;
    let (task_row, nonce, box_port, handle) = preflight(deps, task)?;
    let owner = claim(deps, task)?;
    let worktree = PathBuf::from(task_row.worktree.as_deref().unwrap_or(&task_row.repo));
    let proxy = match listen(deps, task, nonce, &worktree) {
        Ok(proxy) => proxy,
        Err(msg) => {
            say(sink, format_args!("{msg}"));
            give_up(deps, task, &owner);
            return Err(1);
        }
    };
    let tunnel = match deps
        .driver
        .reverse_tunnel(&handle, box_port, proxy.port(), &worktree)
    {
        Ok(tunnel) => tunnel,
        Err(err) => {
            say(
                sink,
                format_args!("cannot open the tunnel into the box: {err}"),
            );
            proxy.shutdown();
            give_up(deps, task, &owner);
            return Err(1);
        }
    };
    let ports = ProxyPorts {
        box_port,
        host_port: proxy.port(),
    };
    match deps
        .store
        .mark_ready(task, &owner, ports, std::process::id(), handle.as_str())
    {
        Ok(Fenced::Applied) => Ok(Running {
            deps,
            task,
            owner,
            proxy,
            tunnel,
        }),
        Ok(Fenced::LostLease) => {
            drop(tunnel);
            proxy.shutdown();
            Err(1)
        }
        Err(err) => {
            say(sink, format_args!("cannot mark the proxy ready: {err}"));
            drop(tunnel);
            proxy.shutdown();
            give_up(deps, task, &owner);
            Err(1)
        }
    }
}

impl Running<'_> {
    /// Heartbeats and checks the Task and tunnel; `None` means keep running.
    pub(super) fn tick(&self) -> Option<Stop> {
        match self.deps.store.heartbeat(self.task, &self.owner) {
            Ok(true) => {}
            Ok(false) => return Some(Stop::LostLease),
            Err(err) => {
                say(&self.deps.stderr, format_args!("heartbeat failed: {err}"));
                return Some(Stop::Failed);
            }
        }
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
        if !self.tunnel.alive() {
            say(
                &self.deps.stderr,
                format_args!("the tunnel into the box exited; stopping"),
            );
            return Some(Stop::TunnelDead);
        }
        None
    }

    /// Stops the tunnel and listener, tidies the row for `stop` and returns the exit code.
    pub(super) fn finish(self, stop: Stop) -> u8 {
        let Self {
            deps,
            task,
            owner,
            proxy,
            tunnel,
        } = self;
        drop(tunnel);
        proxy.shutdown();
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
        u8::from(!matches!(stop, Stop::Closed | Stop::Signal))
    }

    /// The lease owner token.
    #[cfg(test)]
    pub(super) fn owner(&self) -> &str {
        &self.owner
    }

    /// The port the listener is bound to.
    #[cfg(test)]
    pub(super) const fn host_port(&self) -> u16 {
        self.proxy.port()
    }
}
