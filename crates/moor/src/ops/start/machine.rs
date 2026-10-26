//! Holds one of the requester's machines for a new Box whose `.moor.yaml` asks for one. The
//! Proxy supervisor opens the forwards; a start never fails for want of a machine.

use moor_core::config::RepoConfig;
use moor_core::user::UserId;
use moor_core::BoxId;

use crate::store::Store;

/// Claims a machine for Box `id` when `repo` asks for one; the start note when none was held.
pub(super) fn claim(
    store: &Store,
    id: BoxId,
    user: UserId,
    repo: Option<&RepoConfig>,
) -> Option<String> {
    let want = repo?.machine.as_ref()?;
    match store.claim_machine(id, user, want) {
        Ok(pick) => pick.note(),
        Err(e) => Some(format!("machine left out: cannot record it: {e}")),
    }
}
