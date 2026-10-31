//! A provider that names a host, `cloudhypervisor@hv1`, places the Box on a machine of the
//! requester's own. There is no fallback to another provider: a host that cannot take the Box
//! fails the start with the reason.

use moor_core::box_key::PoolSeat;
use moor_core::user::UserId;
use moor_core::{ErrorKind, ProviderId};

use crate::error::CommandError;
use crate::store::Store;

/// The enrolled machine and controller id that place `user`'s Box on `provider`; `None` when `provider` names no
/// host.
///
/// # Errors
/// Refused when `provider` is malformed, or names a machine that is not the user's, has no agent,
/// offers no pool or another provider, or is offline.
pub(crate) fn placement(
    store: &Store,
    user: Option<UserId>,
    provider: &str,
) -> Result<Option<PoolSeat>, CommandError> {
    let id = ProviderId::parse(provider)
        .map_err(|e| CommandError::new(ErrorKind::Refused, e.to_string()))?;
    let Some(host) = id.host() else {
        return Ok(None);
    };
    let refuse = |why: String| Err(CommandError::new(ErrorKind::Refused, why));
    let Some(user) = user else {
        return refuse(format!("a Box with no owner cannot run on host {host}"));
    };
    let Some(found) = store.pool_host(user, host)? else {
        return refuse(format!("{host} is not one of your machines"));
    };
    match found.pool.as_deref() {
        None => return refuse(format!("{host} offers no Box pool: enroll it with --pool")),
        Some(pool) if pool != id.base() => {
            return refuse(format!(
                "{host} offers Boxes on {pool}, not {}: set box.provider to {pool}@{host}",
                id.base()
            ));
        }
        Some(_) => {}
    }
    if !found.online {
        return refuse(format!("{host} is offline: start moor-machine-agent on it"));
    }
    Ok(Some(PoolSeat {
        machine: found.id,
        controller: store.controller_id()?,
    }))
}
