//! What a placement check needs of a user's machine: its pool provider and whether it is up.

use moor_core::user::UserId;

use super::owned_in;
use crate::store::{Store, StoreError};

/// A machine of a user, as a Box pool host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolHost {
    /// The crabbox provider it offers Boxes on; none for an ordinary machine.
    pub(crate) pool: Option<String>,
    /// Whether it was enrolled with `moor-machine-agent`, the only way the controller reaches
    /// a pool host.
    pub(crate) enrolled: bool,
    /// Whether its agent holds a link now.
    pub(crate) online: bool,
}

impl Store {
    /// `user`'s machine `name` as a pool host; `None` when the user has no such machine.
    pub(crate) fn pool_host(
        &self,
        user: UserId,
        name: &str,
    ) -> Result<Option<PoolHost>, StoreError> {
        let conn = self.reader()?;
        let Some((id, machine)) = owned_in(&conn, user)?
            .into_iter()
            .find(|(_, m)| m.name == name)
        else {
            return Ok(None);
        };
        let online = match id {
            None => false,
            Some(id) => conn
                .query_row(
                    "SELECT online FROM machine_links WHERE machine_id = ?",
                    [id],
                    |r| r.get(0),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(false),
                    other => Err(other),
                })?,
        };
        Ok(Some(PoolHost {
            pool: machine.pool,
            enrolled: id.is_some(),
            online,
        }))
    }
}
