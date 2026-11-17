//! The ledger socket a started controller serves: owner-only, and answering the shared egress
//! Proxy only when it presents the egress key.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{config, env_with, Buf};
use crate::controller::drain::Drain;
use crate::controller::{start, Sink};
use crate::proxy::ledger::{EgressLedger, Endpoint, LedgerError};

/// The egress key `env_with` gives every test controller.
const KEY: &str = "test-egress-key";

/// Starts a controller; returns its state dir.
fn served(dir: &tempfile::TempDir) -> PathBuf {
    let cfg = config(dir.path(), "");
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    dir.path().join("state")
}

/// The ledger socket of the state dir `state`, as the egress Proxy presenting `key` dials it.
fn ledger(state: &Path, key: &str) -> EgressLedger {
    EgressLedger::at(
        Endpoint::Unix(state.join("ledger").join("proxy.sock")),
        key.to_owned(),
    )
}

#[test]
fn the_ledger_socket_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let state = served(&dir);
    let socket = state.join("ledger").join("proxy.sock");
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_egress_proxy_is_answered_only_with_the_egress_key() {
    let dir = tempfile::tempdir().unwrap();
    let state = served(&dir);
    assert_eq!(ledger(&state, KEY).token_box("no-such-token"), Ok(None));
    assert!(matches!(
        ledger(&state, &"0".repeat(64)).token_box("no-such-token"),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn the_ledger_keeps_answering_while_the_controller_drains() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path(), "drain_timeout_secs: 30\n");
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let drain = Arc::new(Drain::default());
    let in_flight = drain.hold().expect("held");
    let (wake, signal) = UnixStream::pair().expect("pair");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    let serving = Arc::clone(&drain);
    let served = std::thread::spawn(move || running.serve_until(&wake, &serving, &sink));
    let state = dir.path().join("state");
    let pooled = ledger(&state, KEY);
    assert_eq!(pooled.token_box("x"), Ok(None));

    (&signal).write_all(&[1]).expect("wake");
    while !drain.begun() {
        std::thread::yield_now();
    }
    assert_eq!(
        ledger(&state, KEY).token_box("x"),
        Ok(None),
        "a new connection is answered mid-drain"
    );
    assert_eq!(
        pooled.token_box("x"),
        Ok(None),
        "a pooled connection is answered mid-drain"
    );
    drop(in_flight);
    served.join().expect("drained");
}
