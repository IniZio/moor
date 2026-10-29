//! The `box_machines` row a Box's claim on a machine lives in.

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{params, Connection, OptionalExtension};

use super::ClaimState;
use crate::store::StoreError;

/// A `box_machines` row.
pub(super) struct Claim {
    /// The Box's owner, whose machine it is.
    pub(super) user: UserId,
    /// Whether the Box wants, holds or slept on a machine.
    pub(super) state: ClaimState,
    /// The forwarded ports: moved by the slot once held, as asked while wanted.
    pub(super) ports: Vec<u16>,
    /// What syncs leave alone.
    pub(super) sync_exclude: Vec<String>,
    /// Whether the machine it was given is an enrolled one, so a later machine of the same name
    /// reached over ssh never stands in for it.
    pub(super) enrolled: bool,
}

/// The `box_machines` row of `box_id`, read through `conn`.
pub(super) fn claim_in(conn: &Connection, box_id: BoxId) -> Result<Option<Claim>, StoreError> {
    let sql = "SELECT user, state, machine, labels, slot, ports, sync_exclude, since, until, \
               enrolled FROM box_machines WHERE box = ?";
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
    /// `enrolled`.
    enrolled: bool,
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
            enrolled: r.get(9)?,
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
            enrolled: self.enrolled,
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
pub(super) fn put_in(conn: &Connection, box_id: BoxId, claim: &Claim) -> Result<(), StoreError> {
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
         sync_exclude, since, until, enrolled) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            until,
            claim.enrolled
        ],
    )?;
    Ok(())
}
