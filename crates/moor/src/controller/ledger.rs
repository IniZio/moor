//! The ledger socket's connections: the store calls of the Proxy processes.
//!
//! The ledger stays out of the drain: it accepts and answers until the process exits, so a
//! Box Proxy keeps its lease and secrets for as long as this controller is alive.

use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use super::{log, Sink};
use crate::env::Env;
use crate::vault::Vault;
use crate::Moor;

/// Serves `listener` on a thread of its own until the process exits, with `fronts_key` as the
/// front Proxy's key.
pub(super) fn serve_until_exit(
    listener: UnixListener,
    (env, vault): (&Arc<Env>, &Arc<Vault>),
    fronts_key: Option<Arc<str>>,
    sink: &Sink,
) {
    let (env, vault, sink) = (Arc::clone(env), Arc::clone(vault), Arc::clone(sink));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(e) => {
                    log(&sink, &format!("ledger accept failed: {e}"));
                    continue;
                }
            };
            let (env, vault, key, sink) = (
                Arc::clone(&env),
                Arc::clone(&vault),
                fronts_key.clone(),
                Arc::clone(&sink),
            );
            std::thread::spawn(move || match stream.try_clone() {
                Ok(read) => Moor::serve_ledger(
                    (&env, &vault),
                    key.as_deref(),
                    (BufReader::new(read), stream),
                ),
                Err(e) => log(&sink, &format!("ledger connection setup failed: {e}")),
            });
        }
    });
}
