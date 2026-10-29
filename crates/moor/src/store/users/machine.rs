//! Each user's own machines. A row holds an ssh target and labels, never a credential.

use std::collections::BTreeMap;

use moor_core::config::{EnrolledMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::MachineHoldState;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::store::apply::insert_event;
use crate::store::{Store, StoreError};

mod hold;

use hold::ClaimState;
pub(crate) use hold::MachineWake;

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

    /// Adds the machine an agent enrolled to `user`'s, replacing the key, labels and capacity
    /// of one of the same name; the machine's id, which a replacement keeps.
    pub(crate) fn enroll_machine(
        &self,
        user: UserId,
        machine: &EnrolledMachine,
    ) -> Result<i64, StoreError> {
        let now = self.now().as_millis();
        let labels = serde_json::to_string(&machine.labels)
            .map_err(|e| StoreError::Corrupt(format!("machine labels: {e}")))?;
        Ok(self.writer().query_row(
            "INSERT INTO user_machines_v2 (user, name, pubkey, labels, capacity, enrolled_at) \
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (user, name) DO UPDATE SET \
             pubkey = excluded.pubkey, labels = excluded.labels, capacity = excluded.capacity, \
             enrolled_at = excluded.enrolled_at RETURNING machine_id",
            params![
                user.get(),
                machine.name,
                machine.pubkey,
                labels,
                machine.capacity,
                now
            ],
            |r| r.get(0),
        )?)
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

    /// The distinct ssh targets of every user's machines, sorted.
    pub(crate) fn machine_targets(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT DISTINCT ssh FROM user_machines ORDER BY ssh")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
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
                "UPDATE box_machines SET state = 'asleep' WHERE box = ? AND state = 'held' \
                 RETURNING machine",
                [box_id.get()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(name)
    }

    /// The machine `box_id` holds, in which slot and with which ports; `None` when it holds none,
    /// sleeps or its owner removed it.
    pub(crate) fn box_machine(&self, box_id: BoxId) -> Result<Option<HeldMachine>, StoreError> {
        let held = self.claimed_machine(box_id)?;
        Ok(held.filter(|h| h.state == MachineHoldState::Held))
    }

    /// The machine `box_id` claims, holding a slot, asleep or waiting for one by name; `None`
    /// when it claims none, waits on labels alone, or its owner removed the machine.
    pub(crate) fn claimed_machine(&self, box_id: BoxId) -> Result<Option<HeldMachine>, StoreError> {
        let Some(claim) = claim_in(&self.reader()?, box_id)? else {
            return Ok(None);
        };
        let (name, slot, state) = match claim.state {
            ClaimState::Held { machine, slot } => (machine, slot, MachineHoldState::Held),
            ClaimState::Asleep { machine, last_slot } => {
                (machine, last_slot, MachineHoldState::Asleep)
            }
            ClaimState::Wanted {
                machine: Some(machine),
                ..
            } => (machine, 0, MachineHoldState::Waiting),
            ClaimState::Wanted { machine: None, .. } => return Ok(None),
        };
        let machine = self
            .user_machines(claim.user)?
            .into_iter()
            .find(|m| m.name == name);
        Ok(machine.map(|machine| HeldMachine {
            machine,
            slot,
            ports: claim.ports,
            sync_exclude: claim.sync_exclude,
            state,
        }))
    }
}

/// A `box_machines` row.
struct Claim {
    /// The Box's owner, whose machine it is.
    user: UserId,
    /// Whether the Box wants, holds or slept on a machine.
    state: ClaimState,
    /// The forwarded ports: moved by the slot once held, as asked while wanted.
    ports: Vec<u16>,
    /// What syncs leave alone.
    sync_exclude: Vec<String>,
}

/// The `box_machines` row of `box_id`, read through `conn`.
fn claim_in(conn: &Connection, box_id: BoxId) -> Result<Option<Claim>, StoreError> {
    let sql = "SELECT user, state, machine, labels, slot, ports, sync_exclude, since, until \
               FROM box_machines WHERE box = ?";
    let row = conn
        .query_row(sql, [box_id.get()], |r| Ok(ClaimRow::read(r)))
        .optional()?;
    row.transpose()?.map(ClaimRow::parse).transpose()
}

/// The columns of a `box_machines` row, before the JSON ones are parsed.
struct ClaimRow {
    /// `user`.
    user: i64,
    /// `state`.
    state: String,
    /// `machine`.
    machine: Option<String>,
    /// `labels`, JSON.
    labels: String,
    /// `slot`.
    slot: u16,
    /// `ports`, JSON.
    ports: String,
    /// `sync_exclude`, JSON.
    sync_exclude: String,
    /// `since`, in ms.
    since: i64,
    /// `until`, in ms.
    until: Option<i64>,
}

impl ClaimRow {
    /// The columns `claim_in` selects, in its order.
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            user: r.get(0)?,
            state: r.get(1)?,
            machine: r.get(2)?,
            labels: r.get(3)?,
            slot: r.get(4)?,
            ports: r.get(5)?,
            sync_exclude: r.get(6)?,
            since: r.get(7)?,
            until: r.get(8)?,
        })
    }

    /// The claim the row holds.
    fn parse(self) -> Result<Claim, StoreError> {
        let corrupt =
            |what: &str, e: String| StoreError::Corrupt(format!("box_machines {what}: {e}"));
        let machine = |m: Option<String>| m.ok_or_else(|| corrupt("machine", "missing".to_owned()));
        let state = match self.state.as_str() {
            "wanted" => ClaimState::Wanted {
                labels: from_json("labels", &self.labels)?,
                machine: self.machine,
                since: Ts::from_millis(self.since),
                until: self.until.map(Ts::from_millis),
            },
            "held" => ClaimState::Held {
                machine: machine(self.machine)?,
                slot: self.slot,
            },
            "asleep" => ClaimState::Asleep {
                machine: machine(self.machine)?,
                last_slot: self.slot,
            },
            other => return Err(corrupt("state", other.to_owned())),
        };
        Ok(Claim {
            user: UserId::new(self.user),
            state,
            ports: from_json("ports", &self.ports)?,
            sync_exclude: from_json("sync_exclude", &self.sync_exclude)?,
        })
    }
}

/// The `box_machines` JSON column `what`, holding `v`.
fn from_json<T: serde::de::DeserializeOwned>(what: &str, v: &str) -> Result<T, StoreError> {
    serde_json::from_str(v).map_err(|e| StoreError::Corrupt(format!("box_machines {what}: {e}")))
}

/// `v` as the JSON a `box_machines` column holds.
fn to_json<T: serde::Serialize + ?Sized>(v: &T) -> Result<String, StoreError> {
    serde_json::to_string(v).map_err(|e| StoreError::Corrupt(format!("box_machines json: {e}")))
}

/// Writes `claim` as the `box_machines` row of `box_id`.
fn put_in(conn: &Connection, box_id: BoxId, claim: &Claim) -> Result<(), StoreError> {
    let (state, machine, labels, slot, since, until) = match &claim.state {
        ClaimState::Wanted {
            labels,
            machine,
            since,
            until,
        } => (
            "wanted",
            machine.as_deref(),
            to_json(labels)?,
            0,
            since.as_millis(),
            until.map(Ts::as_millis),
        ),
        ClaimState::Held { machine, slot } => (
            "held",
            Some(machine.as_str()),
            "[]".to_owned(),
            *slot,
            0,
            None,
        ),
        ClaimState::Asleep { machine, last_slot } => (
            "asleep",
            Some(machine.as_str()),
            "[]".to_owned(),
            *last_slot,
            0,
            None,
        ),
    };
    conn.execute(
        "INSERT OR REPLACE INTO box_machines (box, user, state, machine, labels, slot, ports, \
         sync_exclude, since, until) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            box_id.get(),
            claim.user.get(),
            state,
            machine,
            labels,
            slot,
            to_json(&claim.ports)?,
            to_json(&claim.sync_exclude)?,
            since,
            until
        ],
    )?;
    Ok(())
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
    /// Whether the Box holds the slot, sleeps with `slot` and `ports` the ones it last held, or
    /// waits for a slot, with `slot` 0 and `ports` as asked.
    pub(crate) state: MachineHoldState,
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
         WHERE m.user = ? AND m.state = 'held'",
    )?;
    let mut held: BTreeMap<String, BTreeMap<u16, String>> = BTreeMap::new();
    let rows = stmt.query_map([user.get()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    for row in rows {
        let (machine, slot, name): (String, u16, String) = row?;
        held.entry(machine).or_default().insert(slot, name);
    }
    Ok(held)
}
