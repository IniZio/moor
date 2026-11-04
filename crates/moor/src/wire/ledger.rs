//! The ledger socket's connections, served against the controller's shared store.

use std::io::{BufRead, Write};

use crate::env::Env;
use crate::proxy::ledger::{serve, Keys};
use crate::store::Store;
use crate::vault::Vault;

/// Serves one ledger connection; without a shared store it closes at once, so the Proxy
/// sees the controller as unavailable.
pub(crate) fn serve_ledger<G>(
    (env, vault): (&Env, &Vault),
    fronts_key: Option<&str>,
    (read, write): (impl BufRead, impl Write),
    answering: impl FnMut() -> Option<G>,
) {
    if let Some(store) = env.shared::<Store>() {
        let keys = Keys { fronts: fronts_key };
        serve((store, vault), &keys, read, write, answering);
    }
}
