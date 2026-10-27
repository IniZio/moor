//! A Box's claim or release, answered in one write transaction: a new want is granted at once
//! when a slot fits it, kept to wait with a deadline, or refused and rolled back without one.

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::domain::event::{Event, MachineEvent};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::BoxId;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::super::{claim_in, machines_in, put_in, Claim};
use super::{grant_in, wants_in, why_waiting, ClaimState, HoldOp, HoldValue};
use crate::store::apply::insert_event;
use crate::store::{Store, StoreError};

impl Store {
    /// Claims a machine for `id` (idempotent: a Box already holding one gets the same answer) or
    /// releases its claim. `repo` gives the ports and labels a claim leaves out.
    ///
    /// # Errors
    /// A store read or write failed.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the machine_claim tool is its first caller")
    )]
    pub(crate) fn hold_machine(
        &self,
        id: BoxId,
        user: UserId,
        op: &HoldOp,
        repo: Option<&RepoMachine>,
        exclude: &[String],
    ) -> Result<HoldValue, StoreError> {
        if *op == HoldOp::Release {
            return self.release_machine(id);
        }
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (value, keep) = claim_tx(&tx, (id, user), op, (repo, exclude), now)?;
        if keep {
            tx.commit()?;
        } else {
            drop(tx);
        }
        drop(conn);
        Ok(value)
    }

    /// Drops `id`'s claim, recording `machine_detached` when it held a slot.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the machine_claim tool is its first caller")
    )]
    fn release_machine(&self, id: BoxId) -> Result<HoldValue, StoreError> {
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let gone: Option<(Option<String>, String)> = tx
            .query_row(
                "DELETE FROM box_machines WHERE box = ? RETURNING machine, state",
                [id.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (machine, state) = gone.unzip();
        if let (Some(Some(name)), Some("held")) = (&machine, state.as_deref()) {
            let event = MachineEvent::MachineDetached {
                name: name.clone(),
                asleep: false,
            };
            insert_event(&tx, id, &Event::Machine(event), now)?;
        }
        tx.commit()?;
        drop(conn);
        Ok(HoldValue::Released {
            machine: machine.flatten(),
        })
    }
}

/// Runs a claim inside `tx`: the answer, and whether to keep what it wrote.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the machine_claim tool is its first caller")
)]
fn claim_tx(
    tx: &Transaction<'_>,
    (id, user): (BoxId, UserId),
    op: &HoldOp,
    repo: (Option<&RepoMachine>, &[String]),
    now: Ts,
) -> Result<(HoldValue, bool), StoreError> {
    match claim_in(tx, id)?.map(|c| c.state) {
        Some(ClaimState::Held { .. }) => return Ok((held_value(tx, id)?, false)),
        Some(ClaimState::Wanted { .. }) => {}
        _ => match new_want(&machines_in(tx, user)?, user, op, repo, now) {
            Ok(claim) => put_in(tx, id, &claim)?,
            Err(reason) => return Ok((HoldValue::Refused { reason }, false)),
        },
    }
    grant_in(tx, user, now)?;
    let waits = matches!(
        op,
        HoldOp::Claim {
            wait_until: Some(_),
            ..
        }
    );
    Ok(match claim_in(tx, id)? {
        Some(Claim {
            state: ClaimState::Held { .. },
            ..
        }) => (held_value(tx, id)?, true),
        Some(claim) if waits => {
            let reason = why_waiting(tx, &claim)?;
            let position = position_in(tx, id, user)?;
            (HoldValue::Waiting { position, reason }, true)
        }
        Some(claim) => {
            let reason = why_waiting(tx, &claim)?;
            (HoldValue::Refused { reason }, false)
        }
        None => {
            let reason = "the wait for a slot ran out".to_owned();
            (HoldValue::Refused { reason }, true)
        }
    })
}

/// The want `op` records for `user`, given `own`, the user's machines, and the repo's machine
/// section and sync excludes; or why it is refused.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the machine_claim tool is its first caller")
)]
fn new_want(
    own: &[UserMachine],
    user: UserId,
    op: &HoldOp,
    (repo, exclude): (Option<&RepoMachine>, &[String]),
    now: Ts,
) -> Result<Claim, String> {
    let HoldOp::Claim {
        labels,
        machine,
        ports,
        wait_until,
    } = op
    else {
        return Err("a release records no want".to_owned());
    };
    if let Some(m) = machine
        .as_ref()
        .filter(|m| !own.iter().any(|o| &o.name == *m))
    {
        return Err(format!("machine {m} is not one of your machines"));
    }
    let labels = match (labels.is_empty(), machine, repo) {
        (true, None, Some(repo)) => repo.labels.clone(),
        _ => labels.clone(),
    };
    let ports = ports.clone().or_else(|| repo.map(|r| r.ports.clone()));
    let state = ClaimState::Wanted {
        labels,
        machine: machine.clone(),
        since: now,
        until: *wait_until,
    };
    Ok(Claim {
        user,
        state,
        ports: ports.unwrap_or_default(),
        sync_exclude: exclude.to_vec(),
    })
}

/// The `Held` answer for `id`, which holds a slot.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the machine_claim tool is its first caller")
)]
fn held_value(conn: &Connection, id: BoxId) -> Result<HoldValue, StoreError> {
    let claim = claim_in(conn, id)?;
    let Some(Claim {
        user,
        state: ClaimState::Held { machine, slot },
        ports,
        ..
    }) = claim
    else {
        return Err(StoreError::Corrupt("box_machines: no held row".into()));
    };
    let ssh = machines_in(conn, user)?
        .into_iter()
        .find(|m| m.name == machine)
        .map(|m| m.ssh)
        .unwrap_or_default();
    Ok(HoldValue::Held {
        machine,
        ssh,
        slot,
        ports,
    })
}

/// Where `id`'s want stands among `user`'s, counting from 1.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the machine_claim tool is its first caller")
)]
fn position_in(conn: &Connection, id: BoxId, user: UserId) -> Result<usize, StoreError> {
    let wants = wants_in(conn, user)?;
    Ok(wants
        .iter()
        .position(|(w, _)| *w == id)
        .map_or(0, |p| p + 1))
}
