//! Capped resources, counted per machine, that a call holds while it runs.
//!
//! The runner process owns every hold: a hold ends when its [`Held`] drops, so a call that
//! returns, times out, is cancelled or panics frees what it took, and a restart frees all.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::BoxId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::caller::Caller;

/// How often a waiting call repeats what it waits for.
const REMIND: Duration = Duration::from_secs(10);

/// The resources the runner was started with and the holds on them.
#[derive(Default)]
pub(crate) struct Holds {
    /// Units of each resource one machine has.
    caps: BTreeMap<String, usize>,
    /// One pool per machine name and resource, made on first use.
    pools: Mutex<HashMap<(String, String), Arc<Pool>>>,
    /// Source of hold ids, so a dropped hold removes exactly its own holder entry.
    next: AtomicU64,
}

/// The units of one resource on one machine.
struct Pool {
    /// Free units.
    free: Arc<Semaphore>,
    /// Who holds a unit now, by hold id, for the waiting message.
    holders: Mutex<Vec<(u64, BoxId)>>,
}

/// Resource names a call asked for, checked against the runner's resources, in name order.
pub(crate) struct Wanted(BTreeSet<String>);

/// The units a call holds; dropping it frees them.
pub(crate) struct Held {
    /// The units, freed as each drops.
    _units: Vec<Hold>,
}

/// One unit of one resource.
struct Hold {
    /// The unit, returned to its pool on drop.
    _permit: OwnedSemaphorePermit,
    /// Where to clear the holder entry.
    pool: Arc<Pool>,
    /// The holder entry's id.
    id: u64,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut holders = self
            .pool
            .holders
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        holders.retain(|(id, _)| *id != self.id);
    }
}

impl Holds {
    /// Holds over `caps`: the units of each resource name a machine has.
    pub(crate) fn new(caps: BTreeMap<String, usize>) -> Self {
        Self {
            caps,
            ..Self::default()
        }
    }

    /// Checks `names` against the runner's resources, naming the first unknown one.
    pub(crate) fn wanted(&self, names: &[String]) -> Result<Wanted, String> {
        match names.iter().find(|n| !self.caps.contains_key(*n)) {
            Some(name) => Err(format!("unknown resource {name}")),
            None => Ok(Wanted(names.iter().cloned().collect())),
        }
    }

    /// Takes one unit of each wanted resource on the caller's machine, in name order so two
    /// calls never deadlock, waiting at most `wait` in all. While it waits it hands `waiting`
    /// what it waits for; on giving up it returns that reason and holds nothing.
    pub(crate) async fn take<F, Fut>(
        &self,
        caller: &Caller,
        wanted: Wanted,
        wait: Duration,
        mut waiting: F,
    ) -> Result<Held, String>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let deadline = Instant::now() + wait;
        let mut held = Vec::new();
        for name in wanted.0 {
            let pool = self.pool(&caller.machine, &name);
            let permit = match Arc::clone(&pool.free).try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    pool.wait(&name, &caller.machine, deadline, &mut waiting)
                        .await?
                }
            };
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            pool.holders
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((id, caller.box_id));
            held.push(Hold {
                _permit: permit,
                pool,
                id,
            });
        }
        Ok(Held { _units: held })
    }

    /// The pool of `resource` on `machine`; `resource` is one of the runner's.
    fn pool(&self, machine: &str, resource: &str) -> Arc<Pool> {
        let cap = self.caps.get(resource).copied().unwrap_or_default();
        let mut pools = self.pools.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (machine.to_owned(), resource.to_owned());
        Arc::clone(pools.entry(key).or_insert_with(|| {
            Arc::new(Pool {
                free: Arc::new(Semaphore::new(cap)),
                holders: Mutex::default(),
            })
        }))
    }
}

impl Pool {
    /// Waits until `deadline` for a unit, handing `waiting` the reason every [`REMIND`].
    async fn wait<F, Fut>(
        &self,
        resource: &str,
        machine: &str,
        deadline: Instant,
        waiting: &mut F,
    ) -> Result<OwnedSemaphorePermit, String>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = ()>,
    {
        let acquire = Arc::clone(&self.free).acquire_owned();
        tokio::pin!(acquire);
        loop {
            waiting(self.reason(resource, machine)).await;
            let until = (Instant::now() + REMIND).min(deadline);
            match tokio::time::timeout_at(until, &mut acquire).await {
                Ok(permit) => return permit.map_err(|e| e.to_string()),
                Err(_) if Instant::now() >= deadline => return Err(self.reason(resource, machine)),
                Err(_) => {}
            }
        }
    }

    /// What a call waiting for this pool waits for, and who holds it.
    fn reason(&self, resource: &str, machine: &str) -> String {
        let holders = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        let who: Vec<String> = holders
            .iter()
            .map(|(_, b)| format!("Box {}", b.get()))
            .collect();
        format!(
            "waiting for {resource} on {machine}: held by {}",
            who.join(", ")
        )
    }
}
