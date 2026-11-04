//! The ledger socket's connections: the store calls of the Proxy processes.

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use super::drain::{Admitted, Handler};
use super::{log, Sink};
use crate::env::Env;
use crate::vault::Vault;
use crate::Moor;

/// Serves one ledger connection, with `fronts_key` as the front Proxy's key.
pub(super) fn handler(
    (env, vault): (&Arc<Env>, &Arc<Vault>),
    fronts_key: Option<Arc<str>>,
    sink: &Sink,
) -> Handler {
    let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
    Arc::new(
        move |stream: UnixStream, conn: Admitted| match stream.try_clone() {
            Ok(read) => Moor::serve_ledger(
                (&env, &vault),
                fronts_key.as_deref(),
                (BufReader::new(read), stream),
                || conn.answering(),
            ),
            Err(e) => log(&sink, &format!("ledger connection setup failed: {e}")),
        },
    )
}
