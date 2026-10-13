//! Tests for proxy rows, the supervisor lease and proxy events.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::rng::Rng;
use moor_core::secret::Secret;
use moor_core::{TaskId, TaskName};

use super::{Lease, ProxyPorts, BOX_PORT_MAX, BOX_PORT_MIN};
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

const LEASE: Duration = Duration::from_secs(30);
const JUST_UNDER: Duration = Duration::from_millis(29_999);
const NONCE: &[u8] = b"s3cr3t-nonce-value";
/// Lowercase hex of `NONCE`.
const NONCE_HEX: &str = "7333637233742d6e6f6e63652d76616c7565";

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), clock.clone()).unwrap();
    (store, clock)
}

/// Insert a task by name.
fn add(store: &Store, name: &str) -> TaskId {
    let nt = NewTask {
        name: TaskName::parse(name).unwrap(),
        repo: "r".into(),
    };
    store.apply(Decision::insert(nt)).unwrap().task
}

/// A secret holding `bytes`.
fn secret(bytes: &[u8]) -> Secret {
    Secret::new(bytes.to_vec())
}

/// Rng replaying a fixed sequence of draws, then repeating the last.
struct Scripted {
    draws: Vec<u32>,
    next: AtomicUsize,
}

impl Scripted {
    fn new(draws: &[u32]) -> Self {
        Self {
            draws: draws.to_vec(),
            next: AtomicUsize::new(0),
        }
    }
}

impl Rng for Scripted {
    fn u32(&self) -> u32 {
        let i = self.next.fetch_add(1, Ordering::Relaxed);
        let last = self.draws.len().saturating_sub(1);
        self.draws.get(i.min(last)).copied().unwrap_or(0)
    }
}

/// A store with one task that has a reserved proxy row.
fn reserved(dir: &Path) -> (Store, Arc<FakeClock>, TaskId) {
    let (store, clock) = open(dir);
    let id = add(&store, "t");
    store
        .reserve(id, &secret(NONCE), &FakeRng::default())
        .unwrap();
    (store, clock, id)
}

/// Kinds and payloads of the task's events, ascending.
fn events_of(store: &Store, id: TaskId, kind: &str) -> Vec<serde_json::Value> {
    store
        .events(id, 100)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn reserve_returns_port_in_range_and_stores_nonce_unready() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let port = store
        .reserve(id, &secret(NONCE), &FakeRng::default())
        .unwrap();
    assert!((BOX_PORT_MIN..=BOX_PORT_MAX).contains(&port), "{port}");
    let row = store.proxy(id).unwrap().unwrap();
    assert_eq!(row.task, id);
    assert_eq!(row.box_port, port);
    assert_eq!(row.nonce.expose(), NONCE);
    assert!(!row.ready);
    assert_eq!((row.host_port, row.pid, row.handle), (None, None, None));
}

#[test]
fn reserve_skips_port_held_by_another_task() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let held = store
        .reserve(a, &secret(NONCE), &Scripted::new(&[7]))
        .unwrap();
    assert_eq!(held, BOX_PORT_MIN + 7);
    let rng = Scripted::new(&[7, 8]);
    let port = store.reserve(b, &secret(NONCE), &rng).unwrap();
    assert_eq!(port, BOX_PORT_MIN + 8);
}

#[test]
fn rereserve_keeps_port_and_replaces_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let first = store.proxy(id).unwrap().unwrap().box_port;
    let again = store
        .reserve(id, &secret(b"other"), &Scripted::new(&[999]))
        .unwrap();
    assert_eq!(again, first);
    let row = store.proxy(id).unwrap().unwrap();
    assert_eq!(row.box_port, first);
    assert_eq!(row.nonce.expose(), b"other");
}

#[test]
fn reserve_on_unknown_task_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let missing = TaskId::new(id.get() + 100);
    let err = store
        .reserve(missing, &secret(NONCE), &FakeRng::default())
        .unwrap_err();
    assert!(
        matches!(err, StoreError::NotFound(t) if t == missing),
        "{err:?}"
    );
}

#[test]
fn remove_proxy_deletes_row_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    assert!(store.remove_proxy(id).unwrap());
    assert!(store.proxy(id).unwrap().is_none());
    assert!(!store.remove_proxy(id).unwrap());
}

#[test]
fn proxy_row_debug_redacts_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let text = format!("{:?}", store.proxy(id).unwrap().unwrap());
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(!text.contains(NONCE_HEX), "{text}");
    assert!(text.contains("redacted"), "{text}");
}

#[test]
fn second_claimant_fails_while_lease_fresh_and_succeeds_after_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id) = reserved(dir.path());
    assert_eq!(store.claim_lease(id, "A", LEASE).unwrap(), Lease::Claimed);
    assert_eq!(
        store.claim_lease(id, "B", LEASE).unwrap(),
        Lease::Held { owner: "A".into() }
    );
    clock.sleep(LEASE);
    assert_eq!(store.claim_lease(id, "B", LEASE).unwrap(), Lease::Claimed);
    let row = store.proxy(id).unwrap().unwrap();
    assert_eq!(row.lease_owner.as_deref(), Some("B"));
}

#[test]
fn claim_just_before_expiry_is_still_held() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    clock.sleep(JUST_UNDER);
    assert_eq!(
        store.claim_lease(id, "B", LEASE).unwrap(),
        Lease::Held { owner: "A".into() }
    );
}

#[test]
fn owner_reclaims_its_own_fresh_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    assert_eq!(store.claim_lease(id, "A", LEASE).unwrap(), Lease::Claimed);
}

#[test]
fn heartbeat_by_owner_extends_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    clock.sleep(JUST_UNDER);
    assert!(store.heartbeat(id, "A").unwrap());
    clock.sleep(JUST_UNDER);
    assert_eq!(
        store.claim_lease(id, "B", LEASE).unwrap(),
        Lease::Held { owner: "A".into() }
    );
}

#[test]
fn heartbeat_by_non_owner_is_false_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    let before = store.proxy(id).unwrap().unwrap().lease_heartbeat;
    clock.sleep(Duration::from_secs(5));
    assert!(!store.heartbeat(id, "B").unwrap());
    assert_eq!(store.proxy(id).unwrap().unwrap().lease_heartbeat, before);
}

#[test]
fn release_by_owner_lets_another_claim_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    store.release(id, "A").unwrap();
    assert_eq!(store.claim_lease(id, "B", LEASE).unwrap(), Lease::Claimed);
}

#[test]
fn release_by_non_owner_keeps_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    store.claim_lease(id, "A", LEASE).unwrap();
    store.release(id, "B").unwrap();
    assert_eq!(
        store.claim_lease(id, "C", LEASE).unwrap(),
        Lease::Held { owner: "A".into() }
    );
}

#[test]
fn claim_lease_without_a_row_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let err = store.claim_lease(id, "A", LEASE).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(t) if t == id), "{err:?}");
}

#[test]
fn mark_ready_records_ports_and_emits_one_event_without_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let box_port = store.proxy(id).unwrap().unwrap().box_port;
    let ports = ProxyPorts {
        box_port,
        host_port: 41_234,
    };
    store.mark_ready(id, ports, 4242, "tunnel-1").unwrap();
    let row = store.proxy(id).unwrap().unwrap();
    assert!(row.ready);
    assert_eq!(row.host_port, Some(41_234));
    assert_eq!(row.pid, Some(4242));
    assert_eq!(row.handle.as_deref(), Some("tunnel-1"));
    let events = events_of(&store, id, "proxy_started");
    assert_eq!(events.len(), 1);
    let payload = events.first().unwrap();
    assert_eq!(payload["box_port"], box_port);
    assert_eq!(payload["host_port"], 41_234);
    assert_eq!(payload["pid"], 4242);
    assert_eq!(payload["handle"], "tunnel-1");
    assert_no_nonce(payload);
}

/// The payload has no nonce key and its text holds no nonce bytes.
fn assert_no_nonce(payload: &serde_json::Value) {
    let text = payload.to_string();
    assert!(payload.get("nonce").is_none(), "{text}");
    assert!(
        !text.contains("s3cr3t") && !text.contains(NONCE_HEX),
        "{text}"
    );
}

#[test]
fn tunnel_restarted_appends_event_without_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let box_port = store.proxy(id).unwrap().unwrap().box_port;
    store.tunnel_restarted(id, box_port, "ssh exited").unwrap();
    let events = events_of(&store, id, "tunnel_restarted");
    assert_eq!(events.len(), 1);
    let payload = events.first().unwrap();
    assert_eq!(payload["box_port"], box_port);
    assert_eq!(payload["reason"], "ssh exited");
    assert_no_nonce(payload);
}
