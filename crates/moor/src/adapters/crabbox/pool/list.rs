//! Listing a pool host's leases.

use std::collections::BTreeSet;
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{BoxListing, DriverError, HandlesListing};

use super::{Crabbox, PoolHosts};

/// How long a host has to list its leases. A healthy host answers from `crabbox list` well within
/// a second; this is one settle tick, so the controller's periodic pass, which asks every host at
/// once, runs late by at most one tick when a host is slow or away.
pub(in crate::adapters::crabbox) const LIST_WITHIN: Duration = Duration::from_secs(5);

/// What the controller keeps about its pool hosts' lists from one pass to the next.
#[derive(Default)]
pub(crate) struct HostLists(Mutex<Lists>);

/// [`HostLists`]' state.
#[derive(Default)]
struct Lists {
    /// The hosts asked to list whose answer has not come back, late or not.
    asking: BTreeSet<i64>,
    /// The hosts whose last list went unanswered.
    quiet: BTreeSet<i64>,
}

impl HostLists {
    /// The state, held.
    fn lists(&self) -> MutexGuard<'_, Lists> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `machine` may be asked now, marking it asked; a host still on its last list is not.
    fn start(&self, machine: i64) -> bool {
        self.lists().asking.insert(machine)
    }

    /// Marks `machine`'s list done.
    fn done(&self, machine: i64) {
        self.lists().asking.remove(&machine);
    }

    /// The line to log when `machine` answered or not: some only when that changed.
    fn heard(&self, machine: i64, answered: bool, why: impl FnOnce() -> String) -> Option<String> {
        let mut lists = self.lists();
        if answered {
            lists
                .quiet
                .remove(&machine)
                .then(|| format!("pool host {machine} lists its leases again"))
        } else {
            lists.quiet.insert(machine).then(why)
        }
    }
}

/// Not listable: one provider name can be a host of each of several users.
pub(in crate::adapters::crabbox) const fn list(
    _cb: &Crabbox,
    _provider: &ProviderId,
) -> Result<Vec<BoxListing>, DriverError> {
    Err(DriverError::Unsupported(
        "a pool host's leases are not listed by provider",
    ))
}

/// Asks every host holding some of `handles` at once for its leases, waiting `within` for all of
/// them. A host that answers decides its own handles, each asleep when its VM is stopped or
/// paused; one that fails or is late leaves its handles unknown.
///
/// # Errors
/// None of `handles` is a pool Box.
pub(in crate::adapters::crabbox) fn list_handles(
    cb: &Crabbox,
    handles: &[BoxHandle],
    within: Duration,
) -> Result<HandlesListing, DriverError> {
    if !handles.iter().any(super::is_pool) {
        return Err(DriverError::Unsupported("only a pool host lists by handle"));
    }
    let Some(hosts) = cb.pool_hosts.clone() else {
        return Ok(HandlesListing::default());
    };
    let placed: Vec<(&BoxHandle, i64, &str)> = handles
        .iter()
        .filter_map(|h| cb.placed(h).ok().map(|p| (h, p.machine, p.slug)))
        .collect();
    let mut machines: Vec<i64> = placed.iter().map(|(_, m, _)| *m).collect();
    machines.sort_unstable();
    machines.dedup();
    let lists = &cb.host_lists;
    let mut answers = asked(&hosts, lists, &machines, within);
    let mut out = HandlesListing::default();
    for machine in machines {
        let answer = answers
            .iter()
            .position(|(m, _)| *m == machine)
            .map(|at| answers.swap_remove(at).1);
        let said = match answer {
            Some(Ok(leases)) => {
                for (handle, _, slug) in placed.iter().filter(|(_, m, _)| *m == machine) {
                    out.answered.push((*handle).clone());
                    if let Some(lease) = leases.iter().find(|l| l.slug == *slug) {
                        out.listed.push(BoxListing {
                            handle: (*handle).clone(),
                            provider: None,
                            asleep: matches!(lease.state.as_str(), "stopped" | "paused"),
                        });
                    }
                }
                lists.heard(machine, true, String::new)
            }
            Some(Err(why)) => lists.heard(machine, false, || {
                format!("pool host {machine} did not list its leases: {why}")
            }),
            None => lists.heard(machine, false, || {
                format!(
                    "pool host {machine} did not list its leases within {}s",
                    within.as_secs()
                )
            }),
        };
        out.said.extend(said);
    }
    Ok(out)
}

/// The answers of `machines` to a list, each on its own thread, that come within `within` of the
/// ask. A late call runs on and its answer is dropped. A host still on the list it was last asked
/// for is not asked again, so a host that never answers holds one thread, not one a pass.
#[expect(
    clippy::disallowed_methods,
    reason = "bounds a wait on the network in wall time, which a test clock must not stretch"
)]
fn asked(
    hosts: &Arc<dyn PoolHosts>,
    lists: &Arc<HostLists>,
    machines: &[i64],
    within: Duration,
) -> Vec<(i64, Result<Vec<Lease>, String>)> {
    let (tx, rx) = mpsc::channel();
    let mut waiting = 0;
    for &machine in machines.iter().filter(|m| lists.start(**m)) {
        waiting += 1;
        let (tx, hosts, lists) = (tx.clone(), Arc::clone(hosts), Arc::clone(lists));
        std::thread::spawn(move || {
            let leases = match hosts.lease(machine, Verb::List, None, None, &Boot::default()) {
                Ok(Answer::Leases { leases }) => Ok(leases),
                Ok(other) => Err(format!("it answered {other:?}")),
                Err(why) => Err(why),
            };
            lists.done(machine);
            drop(tx.send((machine, leases)));
        });
    }
    drop(tx);
    let by = Instant::now() + within;
    let mut got = Vec::with_capacity(waiting);
    while got.len() < waiting {
        match rx.recv_timeout(by.saturating_duration_since(Instant::now())) {
            Ok(answer) => got.push(answer),
            Err(_) => break,
        }
    }
    got
}
