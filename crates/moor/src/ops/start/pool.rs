//! A provider that names a host, `cloudhypervisor@hv1`, places the Box on a machine of the
//! requester's own. There is no fallback to another provider: a host that cannot take the Box
//! fails the start with the reason.

use moor_core::user::UserId;
use moor_core::{ErrorKind, ProviderId};

use crate::error::CommandError;
use crate::store::Store;

/// Refuses a start whose provider names a host that cannot take the Box.
///
/// # Errors
/// Refused when `provider` is malformed, or names a machine that is not the user's, offers no
/// pool or another provider, or is offline; and, for a usable host, until its driver lands.
pub(super) fn check(store: &Store, user: UserId, provider: &str) -> Result<(), CommandError> {
    let id = ProviderId::parse(provider)
        .map_err(|e| CommandError::new(ErrorKind::Refused, e.to_string()))?;
    let Some(host) = id.host() else {
        return Ok(());
    };
    let refuse = |why: String| Err(CommandError::new(ErrorKind::Refused, why));
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
    refuse(format!(
        "Boxes on your own host ({host}) are not supported yet"
    ))
}
