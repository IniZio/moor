//! The controller's answers to a Box supervisor's calls, each for the Box its hello proved and
//! only while its key is still the Box's.

use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::event::Event;
use moor_core::user::SecretName;
use moor_core::BoxId;
use serde::Serialize;
use zeroize::Zeroizing;

use super::super::hold::hold_of;
use super::super::{BoxView, Call, Fault, MachineWant};
use super::encode;
use super::start;
use crate::store::{ReadyProc, Store, StoreError};
use crate::vault::Vault;

/// `r` as a reply line.
fn reply<T: Serialize>(r: Result<T, StoreError>) -> Zeroizing<String> {
    encode(&r.map_err(Fault::from))
}

/// The reply to `call` from the supervisor of `box_id` that presented `key`.
pub(super) fn answer(
    (store, vault): (&Store, &Vault),
    (box_id, key): (BoxId, &str),
    call: Call,
) -> Zeroizing<String> {
    match store.proxy_key_is(box_id, key.as_bytes()) {
        Ok(true) => {}
        Ok(false) => {
            let replaced = Fault::Refused("another supervisor was started for the Box".to_owned());
            return encode(&Err::<(), _>(replaced));
        }
        Err(e) => return encode(&Err::<(), _>(Fault::from(e))),
    }
    match call {
        Call::BoxMachine
        | Call::Hold
        | Call::SleepMachine
        | Call::GrantMachine
        | Call::WaitingSaid { .. }
        | Call::DetachMachine
        | Call::MachineEvent { .. } => machine(store, box_id, call),
        call => lease_and_tunnel((store, vault), box_id, call),
    }
}

/// The reply to `call` from the shared egress Proxy for `box_id`: only the reads a request makes,
/// the machine it holds among them, and its audit, never the lease or tunnel. Only `View` answers for a closed Box, and
/// only the secrets its spec's rules name are read.
pub(super) fn for_egress(
    (store, vault): (&Store, &Vault),
    box_id: BoxId,
    call: Call,
) -> Zeroizing<String> {
    if let Err(fault) = egress_may(store, box_id, &call) {
        return encode(&Err::<(), _>(fault));
    }
    match call {
        Call::View
        | Call::Spec
        | Call::Ca
        | Call::Secret { .. }
        | Call::OpenLimit { .. }
        | Call::RecordEvent { .. } => lease_and_tunnel((store, vault), box_id, call),
        Call::Hold => machine(store, box_id, call),
        _ => encode(&Err::<(), _>(Fault::Refused(
            "the egress Proxy only reads a Box and records its audit".to_owned(),
        ))),
    }
}

/// Refuses an egress call for a closed Box, other than `View`, and a secret no rule names.
fn egress_may(store: &Store, box_id: BoxId, call: &Call) -> Result<(), Fault> {
    if matches!(call, Call::View) {
        return Ok(());
    }
    if !view(store, box_id)?.is_some_and(|v| v.open) {
        return Err(Fault::Refused(format!("Box {} is closed", box_id.get())));
    }
    if let Call::Secret { name } = call {
        let spec = store.proxy_spec(box_id)?;
        let named = spec.is_some_and(|s| s.rules.iter().any(|r| r.credential == name.as_str()));
        if !named {
            return Err(Fault::Refused(format!(
                "no rule of Box {} names that secret",
                box_id.get()
            )));
        }
    }
    Ok(())
}

/// The open Box whose egress token hashes to `token_hash`; none once it closes.
pub(super) fn token_box(store: &Store, token_hash: &str) -> Result<Option<BoxId>, StoreError> {
    let Some(box_id) = store.egress_token_box(token_hash)? else {
        return Ok(None);
    };
    Ok(view(store, box_id)?.filter(|v| v.open).map(|_| box_id))
}

/// The reply to a call about the proxy row, its lease, its tunnel or its audit.
fn lease_and_tunnel((s, vault): (&Store, &Vault), box_id: BoxId, call: Call) -> Zeroizing<String> {
    match call {
        Call::View => reply(view(s, box_id)),
        Call::BoxPort => reply(s.proxy(box_id).map(|r| r.map(|r| r.box_port))),
        Call::ClaimLease { owner, lease_ms } => {
            reply(s.claim_lease(box_id, &owner, Duration::from_millis(lease_ms)))
        }
        Call::Heartbeat { owner } => reply(s.heartbeat(box_id, &owner)),
        Call::Release { owner } => reply(s.release(box_id, &owner)),
        Call::MarkReady {
            owner,
            box_port,
            pid,
            handle,
            boot_id,
        } => {
            let proc = ReadyProc {
                pid,
                handle: &handle,
                boot_id: boot_id.as_deref(),
            };
            reply(s.mark_ready(box_id, &owner, box_port, proc))
        }
        Call::TunnelOpened => reply(s.tunnel_opened(box_id)),
        Call::NoteTunnelOpened => reply(s.note_tunnel_opened(box_id)),
        Call::TunnelRequest => reply(s.tunnel_request(box_id)),
        Call::TunnelAck => reply(s.tunnel_ack(box_id)),
        Call::AckTunnel { ack } => reply(s.ack_tunnel(box_id, &ack)),
        Call::TunnelRestarted {
            owner,
            box_port,
            reason,
        } => reply(s.tunnel_restarted(box_id, &owner, box_port, &reason)),
        Call::RecordEvent { event } => reply(s.record_proxy_event(box_id, &event)),
        Call::OpenLimit { rule } => reply(s.open_limits(box_id).map(|limits| {
            limits
                .into_iter()
                .find(|l| l.rule == rule)
                .map(|l| l.status)
        })),
        Call::Secret { name } => encode(&secret((s, vault), box_id, &name)),
        Call::RemoveProxy => reply(s.remove_proxy(box_id)),
        Call::Spec => reply(s.proxy_spec(box_id)),
        Call::Ca => encode(&start::ca((s, vault), box_id)),
        _ => encode(&Err::<(), _>(Fault::Refused(
            "a Box's supervisor reads no front lease".to_owned(),
        ))),
    }
}

/// The reply to a call about the machine the Box holds.
fn machine(s: &Store, box_id: BoxId, call: Call) -> Zeroizing<String> {
    match call {
        Call::MachineEvent { owner, event } => {
            reply(s.fenced_event(box_id, &owner, &Event::Machine(event)))
        }
        Call::BoxMachine => reply(s.box_machine(box_id).map(|held| {
            held.map(|h| MachineWant {
                name: h.machine.name,
                ports: h.ports,
                machine_id: h.machine_id,
            })
        })),
        Call::Hold => reply(Ok(hold_of(s, box_id))),
        Call::SleepMachine => reply(s.sleep_machine(box_id)),
        Call::GrantMachine => reply(s.grant_machine(box_id)),
        Call::WaitingSaid { position } => reply(s.waiting_said(box_id, position)),
        Call::DetachMachine => reply(s.detach_machine(box_id)),
        _ => encode(&Err::<(), _>(Fault::Refused(
            "not a machine call".to_owned(),
        ))),
    }
}

/// The Box as its supervisor sees it.
fn view(store: &Store, box_id: BoxId) -> Result<Option<BoxView>, StoreError> {
    Ok(store.box_row(box_id)?.map(|row| BoxView {
        open: !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected),
        handle: row.boxed.as_ref().and_then(|b| b.handle.clone()),
        asleep: row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep),
    }))
}

/// The secret `name` of the Box's user as bytes.
fn secret(
    (store, vault): (&Store, &Vault),
    box_id: BoxId,
    name: &SecretName,
) -> Result<Option<Vec<u8>>, Fault> {
    let user = store
        .box_row(box_id)?
        .and_then(|row| row.user)
        .ok_or_else(|| Fault::Refused(format!("Box {} has no user", box_id.get())))?;
    Ok(store
        .user_secret(vault, user, name)?
        .map(|s| s.expose().to_vec()))
}
