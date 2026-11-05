//! Each user's enrolled machines. A row holds the key the machine's agent signs with and its labels.

use std::collections::BTreeMap;

use moor_core::config::{EnrolledMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::MachineHoldState;
use moor_core::BoxId;
use moor_schema::tables::{BoxMachines, Boxes, UserMachinesV2};
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::apply::insert_event;
use crate::store::db::{self, Db, DbExt as _, OptionalExtension, Tx};
use crate::store::{Store, StoreError};

#[cfg(test)]
mod fixture;
mod hold;
mod links;
mod nodes;
mod row;

use hold::ClaimState;
pub(crate) use hold::MachineWake;
use row::{claim_in, put_in, Claim};

impl Store {
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
        Ok(self.writer().row(
            Query::insert()
                .into_table(UserMachinesV2::Table)
                .columns([
                    UserMachinesV2::User,
                    UserMachinesV2::Name,
                    UserMachinesV2::Pubkey,
                    UserMachinesV2::Labels,
                    UserMachinesV2::Capacity,
                    UserMachinesV2::PoolProvider,
                    UserMachinesV2::EnrolledAt,
                ])
                .values_panic([
                    user.get().into(),
                    machine.name.as_str().into(),
                    machine.pubkey.as_str().into(),
                    labels.into(),
                    machine.capacity.into(),
                    machine.pool.as_deref().into(),
                    now.into(),
                ])
                .on_conflict(
                    OnConflict::columns([UserMachinesV2::User, UserMachinesV2::Name])
                        .update_columns([
                            UserMachinesV2::Pubkey,
                            UserMachinesV2::Labels,
                            UserMachinesV2::Capacity,
                            UserMachinesV2::PoolProvider,
                            UserMachinesV2::EnrolledAt,
                        ])
                        .to_owned(),
                )
                .returning_col(UserMachinesV2::MachineId),
            |r| r.get(0),
        )?)
    }

    /// Removes `user`'s machine `name`; true when there was one.
    pub(crate) fn delete_user_machine(&self, user: UserId, name: &str) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(UserMachinesV2::Table)
                .and_where(Expr::col(UserMachinesV2::User).eq(user.get()))
                .and_where(Expr::col(UserMachinesV2::Name).eq(name)),
        )?;
        Ok(gone == 1)
    }

    /// `user`'s machines by name.
    pub(crate) fn user_machines(&self, user: UserId) -> Result<Vec<UserMachine>, StoreError> {
        machines_in(&self.reader()?, user)
    }

    /// Lets go of the machine `box_id` holds; true when it held one.
    pub(crate) fn detach_machine(&self, box_id: BoxId) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(BoxMachines::Table)
                .and_where(Expr::col(BoxMachines::Box).eq(box_id.get())),
        )?;
        Ok(gone == 1)
    }

    /// Frees the slot of the machine `box_id` holds as the Box sleeps, keeping its claim on the
    /// machine; the machine's name when the Box held a slot.
    pub(crate) fn sleep_machine(&self, box_id: BoxId) -> Result<Option<String>, StoreError> {
        let name = self
            .writer()
            .row(
                Query::update()
                    .table(BoxMachines::Table)
                    .value(BoxMachines::State, "asleep")
                    .and_where(Expr::col(BoxMachines::Box).eq(box_id.get()))
                    .and_where(Expr::col(BoxMachines::State).eq("held"))
                    .returning_col(BoxMachines::Machine),
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
        let machine = named_in(&self.reader()?, claim.user, &name)?;
        Ok(machine.map(|(machine_id, machine)| HeldMachine {
            machine,
            machine_id,
            slot,
            ports: claim.ports,
            sync_exclude: claim.sync_exclude,
            state,
        }))
    }

    /// The name of the enrolled machine `box_id`'s claim is on, when its owner has since removed
    /// it.
    pub(crate) fn removed_machine(&self, box_id: BoxId) -> Result<Option<String>, StoreError> {
        let conn = self.reader()?;
        let Some(claim) = claim_in(&conn, box_id)? else {
            return Ok(None);
        };
        let name = match claim.state {
            ClaimState::Held { machine, .. }
            | ClaimState::Asleep { machine, .. }
            | ClaimState::Wanted {
                machine: Some(machine),
                ..
            } => machine,
            ClaimState::Wanted { machine: None, .. } => return Ok(None),
        };
        let gone = named_in(&conn, claim.user, &name)?.is_none();
        Ok(gone.then_some(name))
    }
}

/// Frees the machine `box_id` holds as the Box leaves the live states, recording
/// `machine_detached`, so a close that no keeper sees still lets the machine go.
pub(in crate::store) fn release_in(tx: &Tx<'_>, box_id: BoxId, now: Ts) -> Result<(), StoreError> {
    let name: Option<String> = tx
        .row(
            Query::delete()
                .from_table(BoxMachines::Table)
                .and_where(Expr::col(BoxMachines::Box).eq(box_id.get()))
                .returning_col(BoxMachines::Machine),
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
    /// Its enrolled id, which its agent links in under.
    pub(crate) machine_id: i64,
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
fn machines_in(conn: &dyn Db, user: UserId) -> Result<Vec<UserMachine>, StoreError> {
    Ok(owned_in(conn, user)?.into_iter().map(|(_, m)| m).collect())
}

/// `user`'s machines by name with the id each is enrolled under.
fn owned_in(conn: &dyn Db, user: UserId) -> Result<Vec<(i64, UserMachine)>, StoreError> {
    links::enrolled_in(conn, user)
}

/// `user`'s machine `name` with its enrolled id.
fn named_in(
    conn: &dyn Db,
    user: UserId,
    name: &str,
) -> Result<Option<(i64, UserMachine)>, StoreError> {
    Ok(owned_in(conn, user)?
        .into_iter()
        .find(|(_, m)| m.name == name))
}

/// `user`'s machines held by a Box: machine name to its taken slots and the Box in each.
fn held_in(
    conn: &dyn Db,
    user: UserId,
) -> Result<BTreeMap<String, BTreeMap<u16, String>>, StoreError> {
    let mut held: BTreeMap<String, BTreeMap<u16, String>> = BTreeMap::new();
    let rows = conn.rows(
        Query::select()
            .column((BoxMachines::Table, BoxMachines::Machine))
            .column((BoxMachines::Table, BoxMachines::Slot))
            .column((Boxes::Table, Boxes::Name))
            .from(BoxMachines::Table)
            .inner_join(
                Boxes::Table,
                Expr::col((Boxes::Table, Boxes::Id)).equals((BoxMachines::Table, BoxMachines::Box)),
            )
            .and_where(Expr::col((BoxMachines::Table, BoxMachines::User)).eq(user.get()))
            .and_where(Expr::col((BoxMachines::Table, BoxMachines::State)).eq("held")),
        |r| Ok::<_, db::Error>((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    for row in rows {
        let (machine, slot, name): (String, u16, String) = row?;
        held.entry(machine).or_default().insert(slot, name);
    }
    Ok(held)
}
