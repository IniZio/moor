//! Listing a pool host's leases.

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_core::name::{BoxHandle, ProviderId};
use moor_core::seam::driver::{BoxListing, DriverError};

use super::{Crabbox, Placed};

/// Not listable: one provider name can be a host of each of several users.
pub(in crate::adapters::crabbox) const fn list(
    _cb: &Crabbox,
    _provider: &ProviderId,
) -> Result<Vec<BoxListing>, DriverError> {
    Err(DriverError::Unsupported(
        "a pool host's leases are not listed by provider",
    ))
}

/// Those of `handles` whose host lists their lease, each asleep when its VM is stopped or
/// paused. A host that cannot be asked leaves its handles out.
pub(in crate::adapters::crabbox) fn list_handles(
    cb: &Crabbox,
    handles: &[BoxHandle],
) -> Vec<BoxListing> {
    let Some(hosts) = cb.pool_hosts.as_deref() else {
        return Vec::new();
    };
    let placed: Vec<(&BoxHandle, Placed<'_>)> = handles
        .iter()
        .filter_map(|h| Some((h, cb.placed(h).ok()?)))
        .collect();
    let mut machines: Vec<i64> = placed.iter().map(|(_, p)| p.machine).collect();
    machines.sort_unstable();
    machines.dedup();
    let mut listed = Vec::new();
    for machine in machines {
        let Ok(Answer::Leases { leases }) =
            hosts.lease(machine, Verb::List, None, None, &Boot::default())
        else {
            continue;
        };
        for (handle, p) in placed.iter().filter(|(_, p)| p.machine == machine) {
            if let Some(lease) = leases.iter().find(|l| l.slug == p.slug) {
                listed.push(BoxListing {
                    handle: (*handle).clone(),
                    provider: None,
                    asleep: matches!(lease.state.as_str(), "stopped" | "paused"),
                });
            }
        }
    }
    listed
}
