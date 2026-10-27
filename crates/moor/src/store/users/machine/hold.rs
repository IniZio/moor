//! A Box's claim on one of its user's machines: wanted, held in a slot, or asleep. Wants are
//! granted first come, first served, in one write transaction per claim or tick.

use std::collections::BTreeMap;

use moor_core::config::{pick_machine, unslot_ports, MachinePick, RepoMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

use super::{claim_in, held_in, machines_in, put_in, Claim};
use crate::store::apply::insert_event;
use crate::store::{Store, StoreError};

mod claim;

/// Where a Box's claim on a machine stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaimState {
    /// Waiting for a free slot on a machine holding every label, or on `machine` alone.
    Wanted {
        /// Labels the machine must all hold; ignored when `machine` names one.
        labels: Vec<String>,
        /// The one machine wanted.
        machine: Option<String>,
        /// When the want was made; older wants are granted first.
        since: Ts,
        /// When the want is dropped; `None` waits for as long as it takes.
        until: Option<Ts>,
    },
    /// Holding `slot` on `machine`.
    Held {
        /// The machine's name.
        machine: String,
        /// The Box's slot on it.
        slot: u16,
    },
    /// The Box sleeps; its slot is free for another Box.
    Asleep {
        /// The machine's name.
        machine: String,
        /// The slot it held before it slept.
        last_slot: u16,
    },
}

/// What a supervisor tick's grant did for its Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachineWake {
    /// The Box wants no machine.
    Nothing,
    /// The Box holds a slot.
    Woke,
    /// Every slot it fits is taken; the want stays.
    Waiting {
        /// The machine's name, or the labels wanted.
        name: String,
        /// Why there is no slot.
        reason: String,
    },
}

impl Store {
    /// Each supervisor tick of an awake Box: an asleep claim is wanted again on the same machine,
    /// then the oldest wants that a free slot fits are granted and expired ones dropped. Only a
    /// Box that waits or slept takes the write lock.
    ///
    /// # Errors
    /// A store read or write failed.
    pub(crate) fn grant_machine(&self, id: BoxId) -> Result<MachineWake, StoreError> {
        match claim_in(&self.reader()?, id)?.map(|c| c.state) {
            None => return Ok(MachineWake::Nothing),
            Some(ClaimState::Held { .. }) => return Ok(MachineWake::Woke),
            Some(_) => {}
        }
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(claim) = claim_in(&tx, id)? else {
            return Ok(MachineWake::Nothing);
        };
        let user = claim.user;
        if let ClaimState::Asleep { machine, last_slot } = claim.state {
            let ports = unslot_ports(&claim.ports, last_slot)
                .map_err(|e| StoreError::Corrupt(format!("box_machines ports: {e}")))?;
            let state = ClaimState::Wanted {
                labels: Vec::new(),
                machine: Some(machine),
                since: now,
                until: None,
            };
            let ports_kept = Claim {
                state,
                ports,
                ..claim
            };
            put_in(&tx, id, &ports_kept)?;
        }
        grant_in(&tx, user, now)?;
        let wake = match claim_in(&tx, id)? {
            None => MachineWake::Nothing,
            Some(Claim {
                state: ClaimState::Held { .. },
                ..
            }) => MachineWake::Woke,
            Some(claim) => {
                let reason = why_waiting(&tx, &claim)?;
                let ClaimState::Wanted {
                    labels, machine, ..
                } = claim.state
                else {
                    return Err(StoreError::Corrupt(
                        "box_machines: asleep after a grant".into(),
                    ));
                };
                let name = machine.unwrap_or_else(|| labels.join(", "));
                MachineWake::Waiting { name, reason }
            }
        };
        tx.commit()?;
        drop(conn);
        Ok(wake)
    }
}

/// Drops `user`'s expired wants, recording `machine_left_out`, then gives each remaining want,
/// oldest first, the slot `pick_machine` finds free.
fn grant_in(conn: &Transaction<'_>, user: UserId, now: Ts) -> Result<(), StoreError> {
    drop_expired_in(conn, user, now)?;
    let own = machines_in(conn, user)?;
    let mut held = held_in(conn, user)?;
    for (id, box_name) in wants_in(conn, user)? {
        let Some(claim) = claim_in(conn, id)? else {
            continue;
        };
        let MachinePick::Attach {
            machine,
            slot,
            ports,
        } = pick_for(&own, &held, &claim)
        else {
            continue;
        };
        held.entry(machine.name.clone())
            .or_default()
            .insert(slot, box_name);
        let state = ClaimState::Held {
            machine: machine.name,
            slot,
        };
        put_in(
            conn,
            id,
            &Claim {
                state,
                ports,
                ..claim
            },
        )?;
    }
    Ok(())
}

/// What `pick_machine` gives the want `claim` among `own`, `held` taken.
fn pick_for(
    own: &[UserMachine],
    held: &BTreeMap<String, BTreeMap<u16, String>>,
    claim: &Claim,
) -> MachinePick {
    let ClaimState::Wanted {
        labels, machine, ..
    } = &claim.state
    else {
        return MachinePick::LeftOut {
            labels: Vec::new(),
            reason: "the Box wants no machine".to_owned(),
        };
    };
    let Some(name) = machine else {
        let want = RepoMachine {
            labels: labels.clone(),
            ports: claim.ports.clone(),
            ..RepoMachine::default()
        };
        return pick_machine(&want, own, held);
    };
    let named: Vec<UserMachine> = own.iter().filter(|m| &m.name == name).cloned().collect();
    if named.is_empty() {
        return MachinePick::LeftOut {
            labels: Vec::new(),
            reason: format!("machine {name} is no longer one of your machines"),
        };
    }
    let want = RepoMachine {
        ports: claim.ports.clone(),
        ..RepoMachine::default()
    };
    pick_machine(&want, &named, held)
}

/// Why the want `claim` has no slot now.
fn why_waiting(conn: &Connection, claim: &Claim) -> Result<String, StoreError> {
    let own = machines_in(conn, claim.user)?;
    let held = held_in(conn, claim.user)?;
    Ok(match pick_for(&own, &held, claim) {
        MachinePick::LeftOut { reason, .. } => reason,
        MachinePick::Attach { machine, .. } => {
            format!("machine {} has a free slot for an older want", machine.name)
        }
    })
}

/// `user`'s wanting Boxes and their names, oldest want first.
fn wants_in(conn: &Connection, user: UserId) -> Result<Vec<(BoxId, String)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT m.box, b.name FROM box_machines m JOIN boxes b ON b.id = m.box \
         WHERE m.user = ? AND m.state = 'wanted' ORDER BY m.since, m.box",
    )?;
    let rows = stmt.query_map([user.get()], |r| Ok((BoxId::new(r.get(0)?), r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Deletes `user`'s wants whose wait ran out by `now`, each with a `machine_left_out` event.
fn drop_expired_in(conn: &Transaction<'_>, user: UserId, now: Ts) -> Result<(), StoreError> {
    let mut stmt = conn.prepare(
        "DELETE FROM box_machines WHERE user = ? AND state = 'wanted' AND until < ? \
         RETURNING box, labels",
    )?;
    let rows = stmt.query_map(params![user.get(), now.as_millis()], |r| {
        Ok((BoxId::new(r.get(0)?), r.get::<_, String>(1)?))
    })?;
    let gone: Vec<(BoxId, String)> = rows.collect::<Result<_, _>>()?;
    drop(stmt);
    for (id, labels) in gone {
        let labels: Vec<String> = serde_json::from_str(&labels)
            .map_err(|e| StoreError::Corrupt(format!("box_machines labels: {e}")))?;
        let reason = "no slot freed before the wait ran out".to_owned();
        let event = MachineEvent::MachineLeftOut { labels, reason };
        insert_event(conn, id, &Event::Machine(event), now)?;
    }
    Ok(())
}
