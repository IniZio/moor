//! What the controller keeps for each open Box: the tunnel that carries the Box's egress
//! forwarder to the shared egress Proxy on this host, and the user's machine the Box holds.
//!
//! One thread per Box ticks every [`TICK`]. It follows the Box's handle, so a Box that gets a new
//! machine gets a new tunnel and new machine forwards on the next tick. It reopens a dead tunnel
//! with backoff, grants and frees the machine's slot, and lets everything go once the Box
//! closes. Nothing outlives the controller: the next controller starts a keeper for each open
//! Box as it starts.
//!
//! A keeper for a Box whose forwarder dials the shared Proxy itself opens no tunnel.

mod host;
mod machine;
#[cfg(test)]
mod tests;
mod tunnel;

use std::collections::HashMap;
use std::fmt::Arguments;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::event::Event;
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::{Driver, TunnelHandle};
use moor_core::{BoxHandle, BoxId};

pub(crate) use host::{KeptProxy, NoProxyHost};
use machine::MachineSlot;

use super::Sink;
use crate::store::{Store, StoreError};

/// Pause between ticks.
pub(crate) const TICK: Duration = Duration::from_secs(2);

/// What every keeper uses.
pub(crate) struct Deps {
    /// The state store.
    pub(crate) store: Store,
    /// Opens tunnels into Boxes.
    pub(crate) driver: Arc<dyn Driver>,
    /// Source of time and sleeping.
    pub(crate) clock: Arc<dyn Clock>,
    /// The shared egress Proxy's socket each Box's tunnel reaches; `None` when Boxes dial it
    /// themselves.
    pub(crate) egress: Option<PathBuf>,
    /// The machine runner's forward socket.
    pub(crate) runner: PathBuf,
    /// Where failures are logged.
    pub(crate) log: Sink,
}

/// The Box as a keeper sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    /// Whether the Box is open.
    open: bool,
    /// Its machine, when it has one.
    handle: Option<BoxHandle>,
    /// Whether it sleeps.
    asleep: bool,
}

/// What one keeper holds between ticks.
struct Inner {
    /// The handle the tunnel and forwards lead into.
    handle: Option<BoxHandle>,
    /// The tunnel; dropping it closes it.
    tunnel: Option<Box<dyn TunnelHandle>>,
    /// Whether the tunnel stays closed because the Box sleeps.
    parked: bool,
    /// Failed opens since the tunnel was last up.
    failures: usize,
    /// When the next open may be tried.
    next_open: Option<Instant>,
    /// Whether a tunnel was up before and died, so its reopening is recorded.
    was_up: bool,
    /// The user's machine attached to the Box.
    machine: MachineSlot,
}

/// One Box's keeper.
pub(crate) struct Kept {
    /// The Box.
    box_id: BoxId,
    /// The Box port its tunnel listens on.
    box_port: u16,
    /// What it uses.
    deps: Arc<Deps>,
    /// Its state; ticks and direct calls take turns.
    inner: Mutex<Inner>,
    /// Set to end its thread.
    stop: AtomicBool,
}

/// Every keeper this controller runs.
#[derive(Default)]
pub(crate) struct Keepers {
    /// By Box.
    kept: Mutex<HashMap<BoxId, Arc<Kept>>>,
}

/// The keepers of this process.
pub(crate) static KEEPERS: LazyLock<Keepers> = LazyLock::new(Keepers::default);

impl Keepers {
    /// The Box's keeper, started on `box_port` when it has none.
    pub(crate) fn start(
        &'static self,
        deps: &Arc<Deps>,
        box_id: BoxId,
        box_port: u16,
    ) -> Arc<Kept> {
        let mut kept = lock(&self.kept);
        if let Some(k) = kept.get(&box_id) {
            return Arc::clone(k);
        }
        let k = Arc::new(Kept::new(Arc::clone(deps), box_id, box_port));
        kept.insert(box_id, Arc::clone(&k));
        drop(kept);
        let ticking = Arc::clone(&k);
        let spawned = std::thread::Builder::new()
            .name(format!("moor-keeper-{}", box_id.get()))
            .spawn(move || {
                while !ticking.stop.load(Ordering::Acquire) && ticking.tick() {
                    ticking.deps.clock.sleep(TICK);
                }
                self.forget(&ticking);
            });
        if let Err(e) = spawned {
            k.say(format_args!("cannot start the keeper: {e}"));
            self.forget(&k);
        }
        k
    }

    /// Ends every keeper, closing its tunnel and forwards, as the controller exits.
    pub(crate) fn end_all(&self) {
        let ids: Vec<BoxId> = lock(&self.kept).keys().copied().collect();
        for id in ids {
            self.end(id);
        }
    }

    /// The Box's keeper, when it runs.
    pub(crate) fn get(&self, box_id: BoxId) -> Option<Arc<Kept>> {
        lock(&self.kept).get(&box_id).cloned()
    }

    /// Ends the Box's keeper, closing its tunnel and forwards.
    pub(crate) fn end(&self, box_id: BoxId) -> Option<Arc<Kept>> {
        let k = lock(&self.kept).remove(&box_id)?;
        k.stop.store(true, Ordering::Release);
        let mut inner = k.lock();
        inner.tunnel = None;
        inner.machine = MachineSlot::default();
        drop(inner);
        Some(k)
    }

    /// Removes `k` once its thread ended, unless another keeper took its place.
    fn forget(&self, k: &Arc<Kept>) {
        let mut kept = lock(&self.kept);
        if kept.get(&k.box_id).is_some_and(|now| Arc::ptr_eq(now, k)) {
            kept.remove(&k.box_id);
        }
    }
}

impl Kept {
    /// A keeper that has not ticked yet.
    fn new(deps: Arc<Deps>, box_id: BoxId, box_port: u16) -> Self {
        let parked = matches!(view(&deps.store, box_id), Ok(Some(v)) if v.asleep);
        Self {
            box_id,
            box_port,
            deps,
            inner: Mutex::new(Inner {
                handle: None,
                tunnel: None,
                parked,
                failures: 0,
                next_open: None,
                was_up: false,
                machine: MachineSlot::default(),
            }),
            stop: AtomicBool::new(false),
        }
    }

    /// The Box port its tunnel listens on.
    pub(crate) const fn box_port(&self) -> u16 {
        self.box_port
    }

    /// One tick; false once the Box closed and everything was let go.
    pub(crate) fn tick(&self) -> bool {
        let view = match view(&self.deps.store, self.box_id) {
            Ok(Some(view)) if view.open => view,
            Ok(_) => {
                self.close();
                return false;
            }
            Err(e) => {
                self.say(format_args!("cannot read the Box: {e}"));
                return true;
            }
        };
        let mut inner = self.lock();
        self.follow(&mut inner, view.handle);
        if inner.parked {
            self.sleep_machine(&mut inner);
        } else {
            drop(self.tend_tunnel(&mut inner));
            self.tend_machine(&mut inner);
        }
        drop(inner);
        true
    }

    /// Whether the Box's credential traffic can reach the shared Proxy now.
    pub(crate) fn up(&self) -> bool {
        let inner = self.lock();
        self.deps.egress.is_none() || inner.tunnel.as_ref().is_some_and(|t| t.alive())
    }

    /// Opens the tunnel now unless it is up or parked; why it is not up after.
    pub(crate) fn bring_up(&self) -> Result<(), String> {
        let handle = view(&self.deps.store, self.box_id)
            .map_err(|e| format!("cannot read the Box: {e}"))?
            .and_then(|v| v.handle);
        let mut inner = self.lock();
        self.follow(&mut inner, handle);
        if inner.parked || self.deps.egress.is_none() {
            return Ok(());
        }
        inner.next_open = None;
        let up = self.tend_tunnel(&mut inner);
        drop(inner);
        up
    }

    /// Closes the tunnel and frees the machine's slot until [`Kept::restore`].
    pub(crate) fn park(&self) {
        let mut inner = self.lock();
        inner.parked = true;
        inner.tunnel = None;
        inner.was_up = false;
        self.sleep_machine(&mut inner);
        drop(inner);
    }

    /// Opens the tunnel again after [`Kept::park`]; the machine's slot is asked for on the next
    /// tick.
    pub(crate) fn restore(&self) -> Result<(), String> {
        self.lock().parked = false;
        self.bring_up()
    }

    /// Lets go of the machine when the Box closed, closes the tunnel and removes the proxy row.
    pub(crate) fn close(&self) {
        let mut inner = self.lock();
        inner.tunnel = None;
        let closed = !matches!(view(&self.deps.store, self.box_id), Ok(Some(v)) if v.open);
        if closed {
            self.let_go_machine(&mut inner);
        }
        inner.machine = MachineSlot::default();
        drop(inner);
        if let Err(e) = self.deps.store.remove_proxy(self.box_id) {
            self.say(format_args!("cannot remove the proxy row: {e}"));
        }
    }

    /// Drops the tunnel and forwards when the Box's handle changed: they lead into a machine
    /// that is gone.
    fn follow(&self, inner: &mut Inner, handle: Option<BoxHandle>) {
        if inner.handle == handle {
            return;
        }
        if let (Some(_), Some(to)) = (&inner.handle, &handle) {
            self.say(format_args!("the Box moved to {}", to.as_str()));
        }
        inner.handle = handle;
        inner.tunnel = None;
        inner.machine = MachineSlot::default();
        inner.failures = 0;
        inner.next_open = None;
        inner.was_up = false;
    }

    /// Appends `event` to the Box's events, reporting a failure.
    fn record(&self, event: &Event) {
        if let Err(e) = self.deps.store.proxy_row_event(self.box_id, event) {
            self.say(format_args!("cannot record {}: {e}", event.kind()));
        }
    }

    /// Writes one `moor keeper: Box N: ` line to the log.
    fn say(&self, args: Arguments<'_>) {
        drop(writeln!(
            self.deps.log.lock().unwrap_or_else(PoisonError::into_inner),
            "moor keeper: Box {}: {args}",
            self.box_id.get()
        ));
    }

    /// The state, whatever a panicking holder left.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }
}

/// The machine runner's forward socket under `state_dir`.
pub(crate) fn runner_socket(state_dir: &Path) -> PathBuf {
    crate::runner_dir::of(state_dir).join(moor_client::forward::SOCKET)
}

/// The Box as a keeper sees it; `None` when it is gone.
fn view(store: &Store, box_id: BoxId) -> Result<Option<View>, StoreError> {
    Ok(store.box_row(box_id)?.map(|row| View {
        open: !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected),
        handle: row.boxed.as_ref().and_then(|b| b.handle.clone()),
        asleep: row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep),
    }))
}

/// `m`, locked, whatever a panicking holder left.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
