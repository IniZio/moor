//! Listing a pool host's leases.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Lease, Verb};
use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::{BoxListing, DriverError, HandlesListing};

use super::{Crabbox, PoolHosts};

/// How long a host has to list its leases. A healthy host answers from `crabbox list` well within
/// a second; this is one settle tick, so the controller's periodic pass, which asks every host at
/// once, runs late by at most one tick when a host is slow or away.
pub(in crate::adapters::crabbox) const LIST_WITHIN: Duration = Duration::from_secs(5);

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
pub(in crate::adapters::crabbox) fn list_handles(
    cb: &Crabbox,
    handles: &[BoxHandle],
    within: Duration,
) -> HandlesListing {
    let Some(hosts) = cb.pool_hosts.clone() else {
        return HandlesListing::default();
    };
    let placed: Vec<(&BoxHandle, i64, &str)> = handles
        .iter()
        .filter_map(|h| cb.placed(h).ok().map(|p| (h, p.machine, p.slug)))
        .collect();
    let mut machines: Vec<i64> = placed.iter().map(|(_, m, _)| *m).collect();
    machines.sort_unstable();
    machines.dedup();
    let mut answers = asked(&hosts, &machines, cb.clock.as_ref(), within);
    let mut out = HandlesListing::default();
    for machine in machines {
        match answers.iter().position(|(m, _)| *m == machine) {
            Some(at) => match answers.swap_remove(at).1 {
                Ok(leases) => {
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
                }
                Err(why) => out.unanswered.push(format!(
                    "pool host {machine} did not list its leases: {why}"
                )),
            },
            None => out.unanswered.push(format!(
                "pool host {machine} did not list its leases within {}s",
                within.as_secs()
            )),
        }
    }
    out
}

/// The answers of `machines` to a list, each on its own thread, that come within `within`. A late
/// call runs on and its answer is dropped.
fn asked(
    hosts: &Arc<dyn PoolHosts>,
    machines: &[i64],
    clock: &dyn Clock,
    within: Duration,
) -> Vec<(i64, Result<Vec<Lease>, String>)> {
    let (tx, rx) = mpsc::channel();
    for &machine in machines {
        let (tx, hosts) = (tx.clone(), Arc::clone(hosts));
        std::thread::spawn(move || {
            let leases = match hosts.lease(machine, Verb::List, None, None, &Boot::default()) {
                Ok(Answer::Leases { leases }) => Ok(leases),
                Ok(other) => Err(format!("it answered {other:?}")),
                Err(why) => Err(why),
            };
            drop(tx.send((machine, leases)));
        });
    }
    drop(tx);
    let by = clock.instant() + within;
    let mut got = Vec::with_capacity(machines.len());
    while got.len() < machines.len() {
        match rx.recv_timeout(by.saturating_duration_since(clock.instant())) {
            Ok(answer) => got.push(answer),
            Err(_) => break,
        }
    }
    got
}
