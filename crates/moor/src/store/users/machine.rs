//! Each user's own machines. A row holds an ssh target and labels, never a credential.

use std::collections::BTreeMap;

use moor_core::config::{pick_machine, wake_slot, MachinePick, RepoMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

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
            "INSERT INTO user_machines (user, name, ssh, labels, capacity, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (user, name) DO UPDATE SET \
             ssh = excluded.ssh, labels = excluded.labels, capacity = excluded.capacity, \
             updated_at = excluded.updated_at",
            params![user.get(), machine.name, machine.ssh, labels, machine.capacity, now, now],
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

    /// Picks one of `user`'s machines and a free slot on it for `box_id` per `want` and records
    /// the outcome: the hold with the patterns its syncs leave alone, or `machine_left_out`. `machine_attached` comes from the supervisor
    /// once the forwards are open. One write transaction, so two starts never share a slot.
    pub(crate) fn claim_machine(
        &self,
        box_id: BoxId,
        user: UserId,
        want: &RepoMachine,
        sync_exclude: &[String],
    ) -> Result<MachinePick, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pick = pick_machine(want, &machines_in(&tx, user)?, &held_in(&tx, user)?);
        match &pick {
            MachinePick::Attach {
                machine,
                slot,
                ports,
            } => {
                let ports = serde_json::to_string(ports)
                    .map_err(|e| StoreError::Corrupt(format!("machine ports: {e}")))?;
                let exclude = serde_json::to_string(sync_exclude)
                    .map_err(|e| StoreError::Corrupt(format!("machine sync exclude: {e}")))?;
                tx.execute(
                    "INSERT OR REPLACE INTO box_machines \
                     (box, user, machine, slot, ports, sync_exclude) VALUES (?, ?, ?, ?, ?, ?)",
                    params![box_id.get(), user.get(), machine.name, slot, ports, exclude],
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

    /// Frees the slot of the machine `box_id` holds as the Box sleeps, keeping its claim on the
    /// machine; the machine's name when the Box held a slot.
    pub(crate) fn sleep_machine(&self, box_id: BoxId) -> Result<Option<String>, StoreError> {
        let name = self
            .writer()
            .query_row(
                "UPDATE box_machines SET asleep = 1 WHERE box = ? AND asleep = 0 RETURNING machine",
                [box_id.get()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(name)
    }

    /// Takes the lowest free slot on the machine asleep `box_id` claims, moving its ports to it.
    /// One write transaction, so a wake and a start never share a slot.
    pub(crate) fn wake_machine(&self, box_id: BoxId) -> Result<MachineWake, StoreError> {
        if !self.claims_asleep(box_id)? {
            return Ok(MachineWake::Nothing);
        }
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(claim) = claim_in(&tx, box_id)?.filter(|c| c.asleep) else {
            return Ok(MachineWake::Nothing);
        };
        let (slot, ports) = match wake_pick(&tx, &claim)? {
            Ok(free) => free,
            Err(reason) => {
                let name = claim.name;
                return Ok(MachineWake::Waiting { name, reason });
            }
        };
        tx.execute(
            "UPDATE box_machines SET asleep = 0, slot = ?, ports = ? WHERE box = ?",
            params![slot, ports, box_id.get()],
        )?;
        tx.commit()?;
        drop(conn);
        Ok(MachineWake::Woke)
    }

    /// Whether `box_id` claims a machine while asleep. Every supervisor tick asks before a wake,
    /// so only an asleep claim takes the write lock.
    fn claims_asleep(&self, box_id: BoxId) -> Result<bool, StoreError> {
        Ok(claim_in(&self.reader()?, box_id)?.is_some_and(|c| c.asleep))
    }

    /// The machine `box_id` holds, in which slot and with which ports; `None` when it holds none,
    /// sleeps or its owner removed it.
    pub(crate) fn box_machine(&self, box_id: BoxId) -> Result<Option<HeldMachine>, StoreError> {
        Ok(self.claimed_machine(box_id)?.filter(|h| !h.asleep))
    }

    /// The machine `box_id` claims, holding a slot or asleep; `None` when it claims none or its
    /// owner removed it.
    pub(crate) fn claimed_machine(&self, box_id: BoxId) -> Result<Option<HeldMachine>, StoreError> {
        let Some(claim) = claim_in(&self.reader()?, box_id)? else {
            return Ok(None);
        };
        let Claim {
            user,
            name,
            slot,
            ports,
            sync_exclude,
            asleep,
        } = claim;
        let machine = self
            .user_machines(user)?
            .into_iter()
            .find(|m| m.name == name);
        Ok(machine.map(|machine| HeldMachine {
            machine,
            slot,
            ports,
            sync_exclude,
            asleep,
        }))
    }
}

/// What waking the machine a sleeping Box claims did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachineWake {
    /// The Box claims no asleep machine.
    Nothing,
    /// The Box holds a slot again.
    Woke,
    /// Every slot is taken; the claim stays asleep.
    Waiting {
        /// The machine's name.
        name: String,
        /// Why there is no slot.
        reason: String,
    },
}

/// A `box_machines` row.
struct Claim {
    /// The Box's owner, whose machine it is.
    user: UserId,
    /// The machine's name.
    name: String,
    /// The slot held, or the last one held while asleep.
    slot: u16,
    /// The forwarded ports, moved by `slot`.
    ports: Vec<u16>,
    /// What syncs leave alone.
    sync_exclude: Vec<String>,
    /// The Box sleeps and holds no slot.
    asleep: bool,
}

/// The lowest free slot on the machine `claim` names and its ports there, as stored; or why
/// there is none.
fn wake_pick(
    conn: &Connection,
    claim: &Claim,
) -> Result<Result<(u16, String), String>, StoreError> {
    let name = &claim.name;
    let machines = machines_in(conn, claim.user)?;
    let Some(machine) = machines.iter().find(|m| &m.name == name) else {
        return Ok(Err(format!(
            "machine {name} is no longer one of your machines"
        )));
    };
    let taken = held_in(conn, claim.user)?.remove(name).unwrap_or_default();
    let (slot, ports) = match wake_slot(machine, &claim.ports, claim.slot, &taken) {
        Ok(free) => free,
        Err(reason) => return Ok(Err(reason)),
    };
    let ports = serde_json::to_string(&ports)
        .map_err(|e| StoreError::Corrupt(format!("machine ports: {e}")))?;
    Ok(Ok((slot, ports)))
}

/// The `box_machines` row of `box_id`, read through `conn`.
fn claim_in(conn: &Connection, box_id: BoxId) -> Result<Option<Claim>, StoreError> {
    let row: Option<(i64, String, u16, String, String, bool)> = conn
        .query_row(
            "SELECT user, machine, slot, ports, sync_exclude, asleep FROM box_machines \
             WHERE box = ?",
            [box_id.get()],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((user, name, slot, ports, exclude, asleep)) = row else {
        return Ok(None);
    };
    let ports: Vec<u16> = serde_json::from_str(&ports)
        .map_err(|e| StoreError::Corrupt(format!("box_machines ports: {e}")))?;
    let sync_exclude: Vec<String> = serde_json::from_str(&exclude)
        .map_err(|e| StoreError::Corrupt(format!("box_machines sync_exclude: {e}")))?;
    Ok(Some(Claim {
        user: UserId::new(user),
        name,
        slot,
        ports,
        sync_exclude,
        asleep,
    }))
}

/// Frees the machine `box_id` holds as the Box leaves the live states, recording
/// `machine_detached`, so a close that no supervisor sees still lets the machine go.
pub(in crate::store) fn release_in(
    tx: &Transaction<'_>,
    box_id: BoxId,
    now: Ts,
) -> Result<(), StoreError> {
    let name: Option<String> = tx
        .query_row(
            "DELETE FROM box_machines WHERE box = ? RETURNING machine",
            [box_id.get()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(name) = name {
        let event = Event::Machine(MachineEvent::MachineDetached {
            name,
            asleep: false,
        });
        insert_event(tx, box_id, &event, now)?;
    }
    Ok(())
}

/// A machine a Box holds: its slot and the ports forwarded, already moved by the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldMachine {
    /// The machine.
    pub(crate) machine: UserMachine,
    /// The Box's slot on it.
    pub(crate) slot: u16,
    /// The forwarded ports, the same number on both ends.
    pub(crate) ports: Vec<u16>,
    /// rsync patterns a sync to the machine leaves alone.
    pub(crate) sync_exclude: Vec<String>,
    /// The Box sleeps: its slot is free and `slot` and `ports` are the ones it last held.
    pub(crate) asleep: bool,
}

/// `user`'s machines by name, read through `conn`.
fn machines_in(conn: &Connection, user: UserId) -> Result<Vec<UserMachine>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT name, ssh, labels, capacity FROM user_machines WHERE user = ? ORDER BY name",
    )?;
    let rows = stmt.query_and_then([user.get()], |r| {
        let (name, ssh, labels): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let capacity: u16 = r.get(3)?;
        let corrupt = |e: String| StoreError::Corrupt(format!("user_machines row {name}: {e}"));
        let labels: Vec<String> =
            serde_json::from_str(&labels).map_err(|e| corrupt(e.to_string()))?;
        UserMachine::parse(&name, &ssh, &labels)
            .and_then(|m| m.with_capacity(capacity))
            .map_err(corrupt)
    })?;
    rows.collect()
}

/// `user`'s machines held by a Box: machine name to its taken slots and the Box in each.
fn held_in(
    conn: &Connection,
    user: UserId,
) -> Result<BTreeMap<String, BTreeMap<u16, String>>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT m.machine, m.slot, b.name FROM box_machines m JOIN boxes b ON b.id = m.box \
         WHERE m.user = ? AND m.asleep = 0",
    )?;
    let mut held: BTreeMap<String, BTreeMap<u16, String>> = BTreeMap::new();
    let rows = stmt.query_map([user.get()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    for row in rows {
        let (machine, slot, name): (String, u16, String) = row?;
        held.entry(machine).or_default().insert(slot, name);
    }
    Ok(held)
}
