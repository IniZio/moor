//! Holds one of the requester's machines for a new Box whose `.moor.yaml` asks for one. The
//! Proxy supervisor opens the forwards; a start never fails for want of a machine.

use moor_core::config::{sync_excludes, RepoConfig, SyncConfig};
use moor_core::user::UserId;
use moor_core::BoxId;

use crate::store::Store;

/// Claims a machine for Box `id` when `repo` asks for one, with what its syncs leave alone per
/// the repo's and the operator's `sync` layers; the start note when none was held.
pub(super) fn claim(
    store: &Store,
    (id, user): (BoxId, UserId),
    repo: Option<&RepoConfig>,
    operator: &SyncConfig,
) -> Option<String> {
    let want = repo?.machine.as_ref()?;
    let exclude = sync_excludes(&want.sync, operator);
    match store.claim_machine(id, user, want, &exclude) {
        Ok(pick) => pick.note(),
        Err(e) => Some(format!("machine left out: cannot record it: {e}")),
    }
}
