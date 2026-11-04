//! The ledger end to end over a real socket: a front Proxy with the right key reads only what
//! a live lease allows, a wrong key or protocol is refused, and a controller that went away
//! is unavailable until it is back.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::user::{Identity, SecretName, UserId};

use super::{serve, FrontsLedger, Keys, LedgerError};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::{put_secret, test_vault_key};
use crate::vault::Vault;

/// A ledger served on a thread, as the controller serves it; dropping it ends the listener
/// and every connection, as a controller that exits does.
pub(crate) struct Served {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    streams: Arc<Mutex<Vec<UnixStream>>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for stream in self.streams.lock().unwrap().drain(..) {
            stream.shutdown(Shutdown::Both).ok();
        }
        drop(UnixStream::connect(&self.socket));
        std::fs::remove_file(&self.socket).ok();
    }
}

/// Serves the ledger of `store` on `socket`, with `fronts` as the front Proxy's key.
pub(crate) fn serve_on(
    socket: &Path,
    store: Store,
    vault: Arc<Vault>,
    fronts: Option<String>,
) -> Served {
    let listener = UnixListener::bind(socket).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let streams = Arc::new(Mutex::new(Vec::new()));
    let (stopped, kept) = (Arc::clone(&stop), Arc::clone(&streams));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            if stopped.load(Ordering::Acquire) {
                return;
            }
            kept.lock().unwrap().push(stream.try_clone().unwrap());
            let (store, vault, fronts) = (store.clone(), Arc::clone(&vault), fronts.clone());
            std::thread::spawn(move || {
                let keys = Keys {
                    fronts: fronts.as_deref(),
                };
                let read = BufReader::new(stream.try_clone().unwrap());
                serve((&store, &vault), &keys, read, stream, || Some(()));
            });
        }
    });
    Served {
        socket: socket.to_owned(),
        stop,
        streams,
    }
}

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Arc<Vault>,
    clock: Arc<FakeClock>,
    ann: UserId,
    socket: PathBuf,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let vault = Vault::load(&test_vault_key(dir.path())).unwrap();
    let (ann, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    put_secret(&store, &vault, ann, "claude", "ann-token");
    let socket = dir.path().join("proxy.sock");
    World {
        _dir: dir,
        store,
        vault: Arc::new(vault),
        clock,
        ann,
        socket,
    }
}

impl World {
    fn serve(&self) -> Served {
        serve_on(
            &self.socket,
            self.store.clone(),
            Arc::clone(&self.vault),
            Some("front-key".to_owned()),
        )
    }

    fn lease(&self, token: &[u8]) {
        let expires = self.clock.now().saturating_add(Duration::from_secs(60));
        self.store
            .add_front_lease(token, "slack", self.ann, expires)
            .unwrap();
    }
}

fn claude() -> SecretName {
    SecretName::parse("claude").unwrap()
}

#[test]
fn a_front_proxy_reads_a_secret_only_while_its_user_holds_a_lease() {
    let w = world();
    let _served = w.serve();
    let ledger = FrontsLedger::at(w.socket.clone(), "front-key".to_owned());
    assert!(
        matches!(
            ledger.secret("slack", w.ann, &claude()),
            Err(LedgerError::Refused(_))
        ),
        "no lease"
    );
    w.lease(b"lease-ann");
    assert_eq!(ledger.lease_user(b"lease-ann", "slack"), Ok(Some(w.ann)));
    assert_eq!(ledger.lease_user(b"lease-ann", "web"), Ok(None));
    let secret = ledger.secret("slack", w.ann, &claude()).unwrap().unwrap();
    assert_eq!(secret.expose(), b"ann-token");
    assert!(
        matches!(
            ledger.secret("web", w.ann, &claude()),
            Err(LedgerError::Refused(_))
        ),
        "a lease of another front"
    );
}

#[test]
fn a_wrong_key_is_refused() {
    let w = world();
    let _served = w.serve();
    w.lease(b"lease-ann");
    let ledger = FrontsLedger::at(w.socket, "not-the-key".to_owned());
    assert!(matches!(
        ledger.lease_user(b"lease-ann", "slack"),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn a_hello_at_another_protocol_is_refused() {
    let w = world();
    let _served = w.serve();
    let mut stream = UnixStream::connect(&w.socket).unwrap();
    writeln!(stream, r#"{{"fronts":{{"v":999,"key":"front-key"}}}}"#).unwrap();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).unwrap();
    assert!(reply.contains("refused"), "{reply}");
    assert!(reply.contains("ledger 999"), "{reply}");
}

#[test]
fn a_controller_that_went_away_is_unavailable_until_it_is_back() {
    let w = world();
    let ledger = FrontsLedger::at(w.socket.clone(), "front-key".to_owned());
    assert!(matches!(
        ledger.lease_user(b"x", "slack"),
        Err(LedgerError::Unavailable(_))
    ));
    let served = w.serve();
    assert_eq!(ledger.lease_user(b"x", "slack"), Ok(None));
    drop(served);
    let _restarted = w.serve();
    assert_eq!(
        ledger.lease_user(b"x", "slack"),
        Ok(None),
        "a pooled connection to the old controller is replaced"
    );
}

#[test]
fn a_call_while_the_controller_is_down_is_unavailable() {
    let w = world();
    let ledger = FrontsLedger::at(w.socket.clone(), "front-key".to_owned());
    let served = w.serve();
    assert_eq!(ledger.lease_user(b"x", "slack"), Ok(None));
    drop(served);
    assert!(matches!(
        ledger.lease_user(b"x", "slack"),
        Err(LedgerError::Unavailable(_))
    ));
}
