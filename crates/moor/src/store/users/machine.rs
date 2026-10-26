//! Each user's own machines. A row holds an ssh target and labels, never a credential.

use moor_core::config::UserMachine;
use moor_core::user::UserId;
use rusqlite::params;

use crate::store::{Store, StoreError};

impl Store {
    /// Adds `machine` to `user`'s list, replacing one of the same name.
    pub(crate) fn set_user_machine(
        &self,
        user: UserId,
        machine: &UserMachine,
    ) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        let labels = serde_json::to_string(&machine.labels)
            .map_err(|e| StoreError::Corrupt(format!("machine labels: {e}")))?;
        self.writer().execute(
            "INSERT INTO user_machines (user, name, ssh, labels, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (user, name) DO UPDATE SET ssh = excluded.ssh, \
             labels = excluded.labels, updated_at = excluded.updated_at",
            params![user.get(), machine.name, machine.ssh, labels, now, now],
        )?;
        Ok(())
    }

    /// Removes `user`'s machine `name`; true when there was one.
    pub(crate) fn delete_user_machine(&self, user: UserId, name: &str) -> Result<bool, StoreError> {
        let gone = self.writer().execute(
            "DELETE FROM user_machines WHERE user = ? AND name = ?",
            params![user.get(), name],
        )?;
        Ok(gone == 1)
    }

    /// `user`'s machines by name.
    pub(crate) fn user_machines(&self, user: UserId) -> Result<Vec<UserMachine>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn
            .prepare("SELECT name, ssh, labels FROM user_machines WHERE user = ? ORDER BY name")?;
        let rows = stmt.query_and_then([user.get()], |r| {
            let (name, ssh, labels): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            let corrupt = |e: String| StoreError::Corrupt(format!("user_machines row {name}: {e}"));
            let labels: Vec<String> =
                serde_json::from_str(&labels).map_err(|e| corrupt(e.to_string()))?;
            UserMachine::parse(&name, &ssh, &labels).map_err(corrupt)
        })?;
        rows.collect()
    }
}
