//! The `box_machines` row a Box's claim on a machine lives in.

use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use moor_schema::tables::BoxMachines;
use sea_query::{Expr, ExprTrait as _, Query};

use super::ClaimState;
use crate::store::db::{Db, DbExt as _, OptionalExtension};
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
}

/// The `box_machines` row of `box_id`, read through `conn`.
pub(super) fn claim_in(conn: &dyn Db, box_id: BoxId) -> Result<Option<Claim>, StoreError> {
    let row = conn
        .row(
            Query::select()
                .columns([
                    BoxMachines::User,
                    BoxMachines::State,
                    BoxMachines::Machine,
                    BoxMachines::Labels,
                    BoxMachines::Slot,
                    BoxMachines::Ports,
                    BoxMachines::SyncExclude,
                    BoxMachines::Since,
                    BoxMachines::Until,
                ])
                .from(BoxMachines::Table)
                .and_where(Expr::col(BoxMachines::Box).eq(box_id.get())),
            |r| Ok(ClaimRow::read(r)),
        )
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
    fn read(r: &crate::store::db::Row<'_>) -> rusqlite::Result<Self> {
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

/// The `state`, `machine`, `labels`, `slot`, `since` and `until` columns that hold a claim.
type Columns<'a> = (&'static str, Option<&'a str>, String, u16, i64, Option<i64>);

/// The columns that hold `state`.
fn columns(state: &ClaimState) -> Result<Columns<'_>, StoreError> {
    Ok(match state {
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
    })
}

/// Writes `claim` as the `box_machines` row of `box_id`.
pub(super) fn put_in(conn: &dyn Db, box_id: BoxId, claim: &Claim) -> Result<(), StoreError> {
    let (state, machine, labels, slot, since, until) = columns(&claim.state)?;
    conn.run(
        Query::delete()
            .from_table(BoxMachines::Table)
            .and_where(Expr::col(BoxMachines::Box).eq(box_id.get())),
    )?;
    // One statement with an OR over both makes SQLite fail to plan against the held-slot index.
    if state == "held" {
        conn.run(
            Query::delete()
                .from_table(BoxMachines::Table)
                .and_where(Expr::col(BoxMachines::State).eq("held"))
                .and_where(Expr::col(BoxMachines::User).eq(claim.user.get()))
                .and_where(Expr::col(BoxMachines::Machine).eq(machine))
                .and_where(Expr::col(BoxMachines::Slot).eq(slot)),
        )?;
    }
    conn.run(
        Query::insert()
            .into_table(BoxMachines::Table)
            .columns([
                BoxMachines::Box,
                BoxMachines::User,
                BoxMachines::State,
                BoxMachines::Machine,
                BoxMachines::Labels,
                BoxMachines::Slot,
                BoxMachines::Ports,
                BoxMachines::SyncExclude,
                BoxMachines::Since,
                BoxMachines::Until,
            ])
            .values_panic([
                box_id.get().into(),
                claim.user.get().into(),
                state.into(),
                machine.into(),
                labels.into(),
                slot.into(),
                to_json(&claim.ports)?.into(),
                to_json(&claim.sync_exclude)?.into(),
                since.into(),
                until.into(),
            ]),
    )?;
    Ok(())
}
