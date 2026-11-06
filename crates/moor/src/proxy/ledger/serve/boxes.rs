//! The controller's answers to the shared egress Proxy's calls for one Box.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::user::SecretName;
use moor_core::BoxId;
use serde::Serialize;
use zeroize::Zeroizing;

use super::super::hold::hold_of;
use super::super::{BoxView, Call, Fault};
use super::encode;
use super::start;
use crate::store::{Store, StoreError};
use crate::vault::Vault;

/// `r` as a reply line.
fn reply<T: Serialize>(r: Result<T, StoreError>) -> Zeroizing<String> {
    encode(&r.map_err(Fault::from))
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
    let s = store;
    match call {
        Call::View => reply(view(s, box_id)),
        Call::Spec => reply(s.proxy_spec(box_id)),
        Call::Ca => encode(&start::ca((s, vault), box_id)),
        Call::Secret { name } => encode(&secret((s, vault), box_id, &name)),
        Call::OpenLimit { rule } => reply(s.open_limits(box_id).map(|limits| {
            limits
                .into_iter()
                .find(|l| l.rule == rule)
                .map(|l| l.status)
        })),
        Call::RecordEvent { event } => reply(s.record_proxy_event(box_id, &event)),
        Call::Hold => reply(Ok(hold_of(s, box_id))),
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

/// The token hash and Box of every open Box with an egress token.
pub(super) fn open_tokens(store: &Store) -> Result<Vec<(String, BoxId)>, StoreError> {
    let mut open = Vec::new();
    for (hash, box_id) in store.egress_tokens()? {
        if view(store, box_id)?.is_some_and(|v| v.open) {
            open.push((hash, box_id));
        }
    }
    Ok(open)
}

/// The Box as the shared egress Proxy sees it.
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
