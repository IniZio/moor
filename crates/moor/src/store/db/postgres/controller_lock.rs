//! The controller lock: a session advisory lock on a connection of its own, which Postgres frees
//! the moment that session ends. It uses no table, so it can be taken before any migration.

use std::time::Duration;

use postgres::fallible_iterator::FallibleIterator as _;
use postgres::Client;

use super::{error, Result, Target};
use crate::store::StoreError;

/// Takes the lock, waiting for it, and names the backend that took it in the same statement.
const TAKE: &str = "SELECT pg_backend_pid(), pg_advisory_lock(hashtext(current_schema()), 2)::text";

/// Takes the lock if it is free. Null, like the lock functions, when no schema is current.
const TRY: &str = "SELECT pg_backend_pid(), pg_try_advisory_lock(hashtext(current_schema()), 2)";

/// The lock's rows in this database and schema, from any session.
const HELD: &str = "FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid = l.pid \
     WHERE l.locktype = 'advisory' AND l.granted AND l.objsubid = 2 AND l.objid = 2 \
     AND l.classid = hashtext(current_schema())::oid \
     AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())";

/// One session of the controller lock, holding it once taken and freeing it when dropped.
pub(crate) struct ControllerLock {
    /// The session.
    client: Client,
    /// Where it connected, for messages and the check from a second session.
    target: Target,
}

impl ControllerLock {
    /// Opens a session on `target`, named `me`.
    pub(crate) fn connect(target: &Target, me: &str) -> Result<Self, StoreError> {
        let target = target.lock_session(me);
        let client = target.connect().map_err(|e| StoreError::Unreachable {
            at: target.shown().to_owned(),
            source: error(e),
        })?;
        Ok(Self { client, target })
    }

    /// Takes the lock if it is free; whether it did. Checked when taken.
    pub(crate) fn try_take(&mut self) -> Result<bool, StoreError> {
        let row = self.client.query_one(TRY, &[]).map_err(db)?;
        let backend: i32 = row.try_get(0).map_err(db)?;
        match row.try_get::<_, Option<bool>>(1).map_err(db)? {
            None => Err(self.not_held()),
            Some(false) => Ok(false),
            Some(true) => self.check(backend).map(|()| true),
        }
    }

    /// Waits for the lock until it is free. Checked when taken.
    pub(crate) fn take(&mut self) -> Result<(), StoreError> {
        let row = self.client.query_one(TAKE, &[]).map_err(db)?;
        let backend: i32 = row.try_get(0).map_err(db)?;
        self.check(backend)
    }

    /// Asks a second session whether `backend` holds the lock. The lock functions do nothing when
    /// no schema is current. Behind a transaction-mode pooler this session has no backend of its
    /// own, so its backend changes, or the second session is handed the same one.
    fn check(&mut self, backend: i32) -> Result<(), StoreError> {
        let again = self.client.query_one("SELECT pg_backend_pid()", &[]);
        let again: i32 = again.and_then(|r| r.try_get(0)).map_err(db)?;
        let mut other = self.target.connect().map_err(db)?;
        let holds = format!("SELECT pg_backend_pid(), count(*) {HELD} AND l.pid = $1");
        let row = other.query_one(&holds, &[&backend]).map_err(db)?;
        let checker: i32 = row.try_get(0).map_err(db)?;
        let rows: i64 = row.try_get(1).map_err(db)?;
        if again == backend && checker != backend && rows == 1 {
            Ok(())
        } else {
            Err(self.not_held())
        }
    }

    /// The refusal for a lock this session should hold and does not.
    fn not_held(&self) -> StoreError {
        StoreError::LockNotHeld {
            db: self.target.shown().to_owned(),
        }
    }

    /// The name of the session holding the lock now.
    pub(crate) fn holder(&mut self) -> Result<Option<String>, StoreError> {
        let holder = format!("SELECT a.application_name {HELD}");
        let row = self.client.query_opt(&holder, &[]).map_err(db)?;
        row.map(|r| r.try_get(0).map_err(db)).transpose()
    }

    /// A session on `target` holding the lock, or the refusal naming who does. It blocks nothing
    /// and leaves this moor free to migrate with no controller able to start.
    pub(crate) fn exclude(target: &Target, me: &str) -> Result<Self, StoreError> {
        let mut lock = Self::connect(target, me)?;
        if lock.try_take()? {
            return Ok(lock);
        }
        let holder = lock.holder()?.unwrap_or_else(|| "?".to_owned());
        Err(StoreError::Leased {
            db: target.shown().to_owned(),
            holder,
        })
    }

    /// Blocks on this session until it closes or stops holding the lock, and says why. It asks
    /// every `every`, so no idle timeout on the way to the server can end a silent session.
    pub(crate) fn until_lost(mut self, every: Duration) -> String {
        loop {
            if let Err(e) = self.client.notifications().timeout_iter(every).next() {
                return format!("the controller lock's connection closed: {e}");
            }
            let holds = format!("SELECT count(*) {HELD} AND l.pid = pg_backend_pid()");
            let rows = self.client.query_one(&holds, &[]);
            match rows.and_then(|r| r.try_get::<_, i64>(0)) {
                Ok(1) => {}
                Ok(_) => return "the controller lock's session no longer holds it".to_owned(),
                Err(e) => return format!("the controller lock's connection closed: {e}"),
            }
        }
    }
}

#[cfg(test)]
impl ControllerLock {
    /// Ends the sessions on `target` named `me`, as a failover does.
    pub(crate) fn terminate(target: &Target, me: &str) {
        target
            .connect()
            .unwrap()
            .query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE application_name = $1",
                &[&me],
            )
            .unwrap();
    }

    /// Has the server end this session once it sits idle for a second.
    pub(crate) fn idle_out(&mut self) {
        self.client
            .batch_execute("SET idle_session_timeout = '1s'")
            .unwrap();
    }

    /// Frees the lock while keeping the session open.
    pub(crate) fn let_go(&mut self) {
        self.client
            .batch_execute("SELECT pg_advisory_unlock_all()")
            .unwrap();
    }
}

/// `e` as the store's error.
fn db(e: postgres::Error) -> StoreError {
    StoreError::Db(error(e))
}
