//! The ledger socket's connections, served against the controller's shared store.

use std::io::{BufRead, Write};

use crate::env::{Env, EGRESS_KEY};
use crate::proxy::adopt;
use crate::proxy::ledger::{serve, Keys};
use crate::store::Store;
use crate::vault::Vault;

/// Serves one ledger connection; without a shared store it closes at once, so the Proxy
/// sees the controller as unavailable.
pub(crate) fn serve_ledger(
    (env, vault): (&Env, &Vault),
    fronts_key: Option<&str>,
    (read, write): (impl BufRead, impl Write),
) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys {
            fronts: fronts_key,
            egress: env.get(EGRESS_KEY).filter(|k| !k.is_empty()),
        };
        serve((store, vault), &keys, read, write);
    }
}

/// Moves the spec and CA files Proxies were started from into their rows; one line per failure.
pub(crate) fn import_proxy_files(env: &Env, vault: &Vault) -> Vec<String> {
    match (env.state_dir(), env.shared::<Store>()) {
        (Some(state_dir), Some(store)) => adopt::import(store, vault, &state_dir),
        _ => Vec::new(),
    }
}
