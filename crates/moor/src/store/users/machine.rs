//! Each user's own machines. A row holds an ssh target and labels, never a credential.

use std::collections::BTreeMap;

use moor_core::config::{pick_machine, MachinePick, RepoMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::store::apply::insert_event;
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
        machines_in(&self.reader()?, user)
    }

    /// Picks one of `user`'s machines for `box_id` per `want` and records the outcome: the hold,
    /// or `machine_left_out`. `machine_attached` comes from the supervisor once the forwards are
    /// open. One write transaction, so two starts never hold the same machine.
    pub(crate) fn claim_machine(
        &self,
        box_id: BoxId,
        user: UserId,
        want: &RepoMachine,
    ) -> Result<MachinePick, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pick = pick_machine(&want.labels, &machines_in(&tx, user)?, &held_in(&tx, user)?);
        match &pick {
            MachinePick::Attach(m) => {
                let ports = serde_json::to_string(&want.ports)
                    .map_err(|e| StoreError::Corrupt(format!("machine ports: {e}")))?;
                tx.execute(
                    "INSERT OR REPLACE INTO box_machines (box, user, machine, ports) \
                     VALUES (?, ?, ?, ?)",
                    params![box_id.get(), user.get(), m.name, ports],
                )?;
            }
            MachinePick::LeftOut { labels, reason } => {
                let event = MachineEvent::MachineLeftOut {
                    labels: labels.clone(),
                    reason: reason.clone(),
                };
                insert_event(&tx, box_id, &Event::Machine(event), now)?;
            }
        }
        tx.commit()?;
        drop(conn);
        Ok(pick)
    }

    /// Lets go of the machine `box_id` holds; true when it held one.
    pub(crate) fn detach_machine(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let gone = self
            .writer()
            .execute("DELETE FROM box_machines WHERE box = ?", [box_id.get()])?;
        Ok(gone == 1)
    }

    /// The machine `box_id` holds and the Box ports it forwards; `None` when it holds none or
    /// its owner removed it.
    pub(crate) fn box_machine(
        &self,
        box_id: BoxId,
    ) -> Result<Option<(UserMachine, Vec<u16>)>, StoreError> {
        let held: Option<(i64, String, String)> = self
            .reader()?
            .query_row(
                "SELECT user, machine, ports FROM box_machines WHERE box = ?",
                [box_id.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((user, name, ports)) = held else {
            return Ok(None);
        };
        let ports: Vec<u16> = serde_json::from_str(&ports)
            .map_err(|e| StoreError::Corrupt(format!("box_machines ports: {e}")))?;
        let user = UserId::new(user);
        let machine = self
            .user_machines(user)?
            .into_iter()
            .find(|m| m.name == name);
        Ok(machine.map(|m| (m, ports)))
    }
}

/// `user`'s machines by name, read through `conn`.
fn machines_in(conn: &Connection, user: UserId) -> Result<Vec<UserMachine>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT name, ssh, labels FROM user_machines WHERE user = ? ORDER BY name")?;
    let rows = stmt.query_and_then([user.get()], |r| {
        let (name, ssh, labels): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let corrupt = |e: String| StoreError::Corrupt(format!("user_machines row {name}: {e}"));
        let labels: Vec<String> =
            serde_json::from_str(&labels).map_err(|e| corrupt(e.to_string()))?;
        UserMachine::parse(&name, &ssh, &labels).map_err(corrupt)
    })?;
    rows.collect()
}

/// `user`'s machines held by an open Box: machine name to Box name.
fn held_in(conn: &Connection, user: UserId) -> Result<BTreeMap<String, String>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT m.machine, b.name FROM box_machines m JOIN boxes b ON b.id = m.box \
         WHERE m.user = ? AND b.state NOT IN ('closed','rejected')",
    )?;
    let rows = stmt.query_map([user.get()], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
