//! What the controller keeps for each open Box: the user's machine the Box holds.
//!
//! One thread per Box ticks every [`TICK`]. It follows the Box's handle, grants and frees the
//! machine's slot, and lets everything go once the Box closes. Nothing outlives the controller:
//! the next controller starts a keeper for each open Box as it starts.
//!
//! The state lock is held only to read or swap state, never across a call into the Box, so
//! [`Kept::park`] answers at once. Calls into the Box take turns on a second lock, which
//! [`Keepers::end`] waits for while the Box is open: once it returns, the ended keeper does
//! nothing.

mod host;
mod machine;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;
use std::fmt::Arguments;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::Event;
use moor_core::domain::hands::{HandsInc, HandsState};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::Driver;
use moor_core::{BoxHandle, BoxId};

pub(crate) use host::{KeptProxy, NoProxyHost};

use super::Sink;
use crate::store::{Store, StoreError};

/// Pause between ticks.
pub(crate) const TICK: Duration = Duration::from_secs(2);

/// What every keeper uses.
pub(crate) struct Deps {
    /// The state store.
    pub(crate) store: Store,
    /// Runs the Box's work.
    pub(crate) driver: Arc<dyn Driver>,
    /// Source of time and sleeping.
    pub(crate) clock: Arc<dyn Clock>,
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
    /// The hands that machine is; none when the Box has none.
    inc: Option<HandsInc>,
    /// Whether it sleeps.
    asleep: bool,
}

/// What one keeper holds between ticks.
struct Inner {
    /// The handle the Box is on.
    handle: Option<BoxHandle>,
    /// The hands that handle is, which each machine write names.
    inc: Option<HandsInc>,
    /// Whether the Box sleeps, so the machine's slot stays free.
    parked: bool,
}

/// One Box's keeper.
pub(crate) struct Kept {
    /// The Box.
    box_id: BoxId,
    /// The Box port its forwarder listens on.
    box_port: u16,
    /// What it uses.
    deps: Arc<Deps>,
    /// Its state; ticks and direct calls take turns.
    inner: Mutex<Inner>,
    /// Held across each call into the Box.
    busy: Mutex<()>,
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

    /// Records `k` as the Box's keeper without starting its thread, for tests that tick it.
    #[cfg(test)]
    pub(crate) fn hold(&self, deps: &Arc<Deps>, box_id: BoxId, box_port: u16) -> Arc<Kept> {
        let k = Arc::new(Kept::new(Arc::clone(deps), box_id, box_port));
        lock(&self.kept).insert(box_id, Arc::clone(&k));
        k
    }

    /// Ends every keeper as the controller exits.
    pub(crate) fn end_all(&self) {
        let all: Vec<Arc<Kept>> = lock(&self.kept).values().cloned().collect();
        for k in &all {
            k.stop.store(true, Ordering::Release);
        }
        for k in all {
            self.end(k.box_id);
        }
    }

    /// The Box's keeper, when it runs.
    pub(crate) fn get(&self, box_id: BoxId) -> Option<Arc<Kept>> {
        lock(&self.kept).get(&box_id).cloned()
    }

    /// Ends the Box's keeper. While the Box is open it first waits for any call the keeper makes
    /// into the Box, so a keeper started after is not overtaken by this one; a closed Box gets
    /// no later keeper, so its end does not wait.
    pub(crate) fn end(&self, box_id: BoxId) -> Option<Arc<Kept>> {
        let k = lock(&self.kept).remove(&box_id)?;
        k.stop.store(true, Ordering::Release);
        let closed = matches!(view(&k.deps.store, box_id), Ok(Some(v)) if !v.open);
        let busy = (!closed).then(|| lock(&k.busy));
        drop(busy);
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
                inc: None,
                parked,
            }),
            busy: Mutex::new(()),
            stop: AtomicBool::new(false),
        }
    }

    /// The Box port its forwarder listens on.
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
        let busy = lock(&self.busy);
        if self.stopped() {
            return false;
        }
        let parked = self.follow(view.handle, view.inc);
        if parked {
            self.sleep_machine();
        } else {
            self.grant_machine();
        }
        drop(busy);
        true
    }

    /// Whether the keeper was ended.
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// Whether the Box sleeps, or is about to, so the keeper holds nothing for it.
    pub(crate) fn parked(&self) -> bool {
        self.lock().parked
    }

    /// Frees the machine's slot until [`Kept::restore`].
    pub(crate) fn park(&self) {
        self.lock().parked = true;
        self.sleep_machine();
    }

    /// Asks for the machine's slot again after [`Kept::park`], on the next tick.
    pub(crate) fn restore(&self) {
        self.lock().parked = false;
    }

    /// Lets go of the machine when the Box closed and removes the proxy row.
    pub(crate) fn close(&self) {
        let closed = !matches!(view(&self.deps.store, self.box_id), Ok(Some(v)) if v.open);
        if closed {
            self.let_go_machine();
        }
        if let Err(e) = self.deps.store.remove_proxy(self.box_id) {
            self.say(format_args!("cannot remove the proxy row: {e}"));
        }
    }

    /// Follows the Box to its new handle. Whether the Box is parked.
    fn follow(&self, handle: Option<BoxHandle>, inc: Option<HandsInc>) -> bool {
        let mut inner = self.lock();
        let parked = inner.parked;
        inner.inc = inc;
        if inner.handle == handle {
            return parked;
        }
        if let (Some(_), Some(to)) = (&inner.handle, &handle) {
            self.say(format_args!("the Box moved to {}", to.as_str()));
        }
        inner.handle = handle;
        parked
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

/// The Box as a keeper sees it; `None` when it is gone.
fn view(store: &Store, box_id: BoxId) -> Result<Option<View>, StoreError> {
    Ok(store.box_row(box_id)?.map(|row| View {
        open: !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected),
        handle: row.hands.as_ref().and_then(|b| b.handle.clone()),
        inc: row.hands.as_ref().map(|b| b.inc),
        asleep: row
            .hands
            .as_ref()
            .is_some_and(|b| b.state == HandsState::Asleep),
    }))
}

/// `m`, locked, whatever a panicking holder left.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
