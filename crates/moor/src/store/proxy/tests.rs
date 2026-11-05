//! Tests for proxy rows and their egress token.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::rng::Rng;
use moor_core::{BoxId, BoxName};
use moor_schema::tables::Proxy;
use sea_query::{Expr, ExprTrait as _, Query};

use super::{BOX_PORT_MAX, BOX_PORT_MIN, EGRESS_PORT_MAX, EGRESS_PORT_MIN};
use crate::store::db::DbExt as _;
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;

/// Open a store in a temp dir with a fake clock.
fn open(dir: &Path) -> (Store, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.join("moor.db"), clock.clone());
    (store, clock)
}

/// Insert a box by name.
fn add(store: &Store, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
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

/// A store with one box that has a reserved proxy row.
fn reserved(dir: &Path) -> (Store, Arc<FakeClock>, BoxId) {
    let (store, clock) = open(dir);
    let id = add(&store, "t");
    store.reserve(id, &FakeRng::default()).unwrap();
    (store, clock, id)
}

#[test]
fn reserve_returns_port_in_range() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let port = store.reserve(id, &FakeRng::default()).unwrap();
    assert!((BOX_PORT_MIN..=BOX_PORT_MAX).contains(&port), "{port}");
    let row = store.proxy(id).unwrap().unwrap();
    assert_eq!(row.box_id, id);
    assert_eq!(row.box_port, port);
}

#[test]
fn reserve_skips_port_held_by_another_box() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let held = store.reserve(a, &Scripted::new(&[7])).unwrap();
    assert_eq!(held, BOX_PORT_MIN + 7);
    let rng = Scripted::new(&[7, 8]);
    let port = store.reserve(b, &rng).unwrap();
    assert_eq!(port, BOX_PORT_MIN + 8);
}

#[test]
fn reserve_gives_each_box_its_own_egress_port_outside_the_box_ports() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let _ = store.reserve(a, &Scripted::new(&[7])).unwrap();
    let _ = store.reserve(b, &Scripted::new(&[7, 8])).unwrap();
    let (ra, rb) = (
        store.proxy(a).unwrap().unwrap(),
        store.proxy(b).unwrap().unwrap(),
    );
    for row in [&ra, &rb] {
        assert!(
            (EGRESS_PORT_MIN..=EGRESS_PORT_MAX).contains(&row.egress_port),
            "{row:?}"
        );
    }
    assert_ne!(ra.egress_port, rb.egress_port);
}

#[test]
fn no_forward_port_is_another_rows_egress_or_forward_port() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let a = add(&store, "a");
    let b = add(&store, "b");
    let _ = store.reserve(a, &Scripted::new(&[7])).unwrap();
    let _ = store.reserve(b, &Scripted::new(&[7])).unwrap();
    let (ra, rb) = (
        store.proxy(a).unwrap().unwrap(),
        store.proxy(b).unwrap().unwrap(),
    );
    let ports = [
        ra.egress_port,
        ra.forward_port.unwrap(),
        rb.egress_port,
        rb.forward_port.unwrap(),
    ];
    for port in ports {
        assert!(
            (EGRESS_PORT_MIN..=EGRESS_PORT_MAX).contains(&port),
            "{ports:?}"
        );
        assert_eq!(ports.iter().filter(|p| **p == port).count(), 1, "{ports:?}");
    }
}

#[test]
fn rereserve_keeps_the_forward_port() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let first = store.proxy(id).unwrap().unwrap().forward_port;
    assert!(first.is_some());
    let _ = store.reserve(id, &Scripted::new(&[999])).unwrap();
    assert_eq!(store.proxy(id).unwrap().unwrap().forward_port, first);
}

#[test]
fn rereserve_keeps_the_egress_port() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    let first = store.proxy(id).unwrap().unwrap().egress_port;
    let _ = store.reserve(id, &Scripted::new(&[999])).unwrap();
    assert_eq!(store.proxy(id).unwrap().unwrap().egress_port, first);
}

#[test]
fn reserve_on_unknown_box_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let id = add(&store, "t");
    let missing = BoxId::new(id.get() + 100);
    let err = store.reserve(missing, &FakeRng::default()).unwrap_err();
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
fn two_rows_cannot_hold_one_forward_port() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = open(dir.path());
    let (a, b) = (add(&store, "a"), add(&store, "b"));
    for id in [a, b] {
        let _ = store.reserve(id, &FakeRng::default()).unwrap();
    }
    let taken = store.proxy(a).unwrap().unwrap().forward_port.unwrap();
    let err = store
        .writer()
        .run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::ForwardPort, taken)
                .and_where(Expr::col(Proxy::Box).eq(b.get())),
        )
        .unwrap_err();
    assert!(super::forward_port_taken(&err), "{err}");
}

#[test]
fn a_reserve_again_needs_the_token_delivered_again() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = reserved(dir.path());
    assert!(store.set_egress_token(id, b"t1").unwrap());
    assert!(store.note_egress_token_delivered(id, b"t1", 1).unwrap());
    let _ = store.reserve(id, &FakeRng::default()).unwrap();
    assert!(!store.egress_token_delivered(id, 1).unwrap());
}

/// A Box acquired as `p/first`, with a reserved proxy row whose egress token was delivered.
fn delivered_on_a_machine(dir: &Path) -> (Store, Arc<FakeClock>, BoxId) {
    use moor_core::domain::box_state::BoxLifecycleInput;
    use moor_core::domain::boxes::BoxInput;
    use moor_core::domain::decision::{Field, Fields};
    use moor_core::ProviderId;
    let (store, clock, id) = reserved(dir);
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Admitted)
                .box_input(BoxInput::Requested)
                .set(Fields {
                    box_provider: Field::Set(ProviderId::new("p")),
                    ..Fields::default()
                }),
        )
        .unwrap();
    acquire(&store, id, "p/first");
    assert!(store.set_egress_token(id, b"t1").unwrap());
    assert!(store.note_egress_token_delivered(id, b"t1", 1).unwrap());
    (store, clock, id)
}

/// Records Box `id` acquired as `handle`, as a start or a retry onto a fresh machine does.
fn acquire(store: &Store, id: BoxId, handle: &str) {
    use moor_core::domain::boxes::BoxInput;
    store
        .apply(Decision::new(id).box_input(BoxInput::Acquired {
            handle: moor_core::BoxHandle::new(handle),
        }))
        .unwrap();
}

#[test]
fn a_box_acquired_again_on_a_fresh_machine_needs_the_token_delivered_again() {
    use moor_core::domain::boxes::BoxInput;
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = delivered_on_a_machine(dir.path());
    for input in [BoxInput::DriverMissing, BoxInput::Requested] {
        store.apply(Decision::new(id).box_input(input)).unwrap();
    }
    acquire(&store, id, "p/second");
    assert!(!store.egress_token_delivered(id, 1).unwrap());
}

#[test]
fn a_box_moved_onto_a_fork_needs_the_token_delivered_again() {
    use moor_core::domain::decision::{Field, Fields};
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = delivered_on_a_machine(dir.path());
    store
        .apply(Decision::new(id).set(Fields {
            box_handle: Field::Set(moor_core::BoxHandle::new("p/fork")),
            ..Fields::default()
        }))
        .unwrap();
    assert!(!store.egress_token_delivered(id, 1).unwrap());
}

#[test]
fn a_change_that_keeps_the_machine_keeps_the_delivery() {
    use moor_core::domain::decision::{Field, Fields};
    let dir = tempfile::tempdir().unwrap();
    let (store, _, id) = delivered_on_a_machine(dir.path());
    store
        .apply(Decision::new(id).set(Fields {
            box_workdir: Field::Set("/work".to_owned()),
            ..Fields::default()
        }))
        .unwrap();
    assert!(store.egress_token_delivered(id, 1).unwrap());
}
