//! The ledger socket a started controller serves: owner-only, and answering the front Proxy
//! only when it presents the key this controller started it with.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::io::Write as _;
use std::os::unix::net::UnixStream;

use super::{config, env_with, Buf};
use crate::controller::drain::Drain;
use crate::controller::{start, Sink};
use crate::proxy::ledger::{FrontsLedger, LedgerError};

/// Starts a controller with config `cfg`; returns its state dir and the front Proxy's key.
fn served(dir: &tempfile::TempDir, cfg: &str) -> (PathBuf, Option<String>) {
    let cfg = config(dir.path(), cfg);
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let key = running.ledger.1.as_deref().map(str::to_owned);
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    (dir.path().join("state"), key)
}

#[test]
fn the_ledger_socket_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = served(&dir, "");
    let socket = state.join("ledger").join("proxy.sock");
    let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_front_proxy_is_answered_only_with_the_key_it_was_started_with() {
    let dir = tempfile::tempdir().unwrap();
    let (state, key) = served(&dir, "fronts:\n  slack:\n    proxy_rules: [claude]\n");
    let key = key.expect("a front Proxy runs");
    let ledger = FrontsLedger::new(&state, key);
    assert_eq!(ledger.lease_user(b"no-such-lease", "slack"), Ok(None));
    let wrong = FrontsLedger::new(&state, "0".repeat(64));
    assert!(matches!(
        wrong.lease_user(b"no-such-lease", "slack"),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn without_a_front_proxy_no_key_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let (state, key) = served(&dir, "fronts:\n  slack: {}\n");
    assert_eq!(key, None);
    let ledger = FrontsLedger::new(&state, String::new());
    assert!(matches!(
        ledger.lease_user(b"no-such-lease", "slack"),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn the_ledger_keeps_answering_while_the_controller_drains() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(
        dir.path(),
        "drain_timeout_secs: 30\nfronts:\n  slack:\n    proxy_rules: [claude]\n",
    );
    let running = start(&env_with(dir.path(), &[]), &cfg).expect("start");
    let key = running.ledger.1.as_deref().map(str::to_owned).expect("key");
    let drain = Arc::new(Drain::default());
    let in_flight = drain.hold().expect("held");
    let (wake, signal) = UnixStream::pair().expect("pair");
    let sink: Sink = Arc::new(Mutex::new(Buf::default()));
    let serving = Arc::clone(&drain);
    let served = std::thread::spawn(move || running.serve_until(&wake, &serving, &sink));
    let state = dir.path().join("state");
    let ledger = FrontsLedger::new(&state, key.clone());
    assert_eq!(ledger.lease_user(b"no-such-lease", "slack"), Ok(None));

    (&signal).write_all(&[1]).expect("wake");
    while !drain.begun() {
        std::thread::yield_now();
    }
    assert_eq!(
        FrontsLedger::new(&state, key).lease_user(b"x", "slack"),
        Ok(None),
        "a new connection is answered mid-drain"
    );
    assert_eq!(
        ledger.lease_user(b"no-such-lease", "slack"),
        Ok(None),
        "a pooled connection is answered mid-drain"
    );
    drop(in_flight);
    served.join().expect("drained");
}
