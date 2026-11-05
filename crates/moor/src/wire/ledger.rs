//! The ledger socket's connections, served against the controller's shared store.

use std::io::{Read, Write};

use crate::env::{Env, EGRESS_KEY};
use crate::proxy::ledger::{serve, Keys};
use crate::store::Store;
use crate::vault::Vault;

/// Serves one connection on the ledger socket, to any Proxy; without a shared store it closes
/// at once, so the Proxy sees the controller as unavailable.
pub(crate) fn serve_ledger(
    (env, vault): (&Env, &Vault),
    fronts_key: Option<&str>,
    stream: impl Read + Write,
) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            supervisors: true,
            fronts: fronts_key,
            egress: egress_key(env),
        };
        serve((store, vault), &keys, stream);
    }
}

/// Serves one connection on the ledger's TLS listener, to the shared egress Proxy only.
pub(crate) fn serve_egress_ledger((env, vault): (&Env, &Vault), stream: impl Read + Write) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            supervisors: false,
            fronts: None,
            egress: egress_key(env),
        };
        serve((store, vault), &keys, stream);
    }
}

/// The shared egress Proxy's key; none while it is off.
fn egress_key(env: &Env) -> Option<&str> {
    env.get(EGRESS_KEY).filter(|k| !k.is_empty())
}
