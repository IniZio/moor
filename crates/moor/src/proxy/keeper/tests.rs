//! A Box's keeper against the store and a fake driver: the machine it holds is freed while the
//! Box sleeps and let go with the Box.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::user::Identity;
use moor_core::{BoxHandle, BoxId, BoxName, HoldOp, ProviderId};

use super::{Deps, Kept};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;

/// A store with one ready Box, a fake driver and a keeper for the Box.
struct World {
    dir: tempfile::TempDir,
    store: Store,
    driver: Arc<FakeDriver>,
    id: BoxId,
    log: Arc<Mutex<Vec<u8>>>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let driver = Arc::new(FakeDriver::new());
        let id = ready_box(&store, &driver);
        store.reserve(id, &FakeRng::default()).unwrap();
        Self {
            dir,
            store,
            driver,
            id,
            log: Arc::default(),
        }
    }

    fn kept(&self) -> Kept {
        let deps = Deps {
            store: self.store.clone(),
            driver: Arc::clone(&self.driver) as Arc<dyn Driver>,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            log: self.log.clone(),
        };
        Kept::new(Arc::new(deps), self.id, 41_000)
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("egress.sock")
    }

    fn handle(&self) -> BoxHandle {
        let row = self.store.box_row(self.id).unwrap().unwrap();
        row.hands.unwrap().handle.unwrap()
    }

    fn close_box(&self) {
        let d = Decision::new(self.id)
            .lifecycle(BoxLifecycleInput::Close { reason: None })
            .session(SessionInput::End)
            .hands_input(self.inc(), HandsInput::Released);
        self.store.apply(d).unwrap();
    }

    /// The Box's current hands.
    fn inc(&self) -> HandsInc {
        let row = self.store.box_row(self.id).unwrap().unwrap();
        row.hands_inc()
    }

    fn kinds(&self) -> Vec<String> {
        let events = self.store.events(self.id, 50).unwrap().into_iter();
        events
            .map(|e| e.kind)
            .filter(|k| k.starts_with("machine_"))
            .collect()
    }
}

/// A fresh machine from `driver` named after `name`.
fn acquire(driver: &FakeDriver, id: BoxId, name: &str) -> BoxHandle {
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new("p"),
            box_id: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    got.handle
}

/// Inserts a Box and takes it to Ready on a machine from `driver`.
pub(crate) fn ready_box(store: &Store, driver: &FakeDriver) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
        spec: SpecFields::default(),
    };
    let id = store.apply(Decision::insert(new)).unwrap().box_id;
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(
            HandsInc::FIRST,
            HandsFields {
                provider: Field::Set(ProviderId::new("p")),
                ..HandsFields::default()
            },
        );
    store.apply(admitted).unwrap();
    let handle = acquire(driver, id, "demo");
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle });
    store.apply(ready).unwrap();
    id
}

#[test]
fn a_closed_box_ends_its_keeper_and_loses_its_proxy_row() {
    let w = World::new();
    let kept = w.kept();
    assert!(kept.tick());
    w.close_box();
    assert!(!kept.tick());
    assert!(w.store.proxy(w.id).unwrap().is_none());
}

/// The Box's requester holds the machine `mini`.
fn hold_machine(w: &World) {
    let (user, _) = w
        .store
        .enroll(&Identity::parse("herdr:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &["macos".to_owned()]).unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    };
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
}

#[test]
fn the_machine_is_let_go_when_the_box_closes() {
    let w = World::new();
    hold_machine(&w);
    let kept = w.kept();
    assert!(kept.tick());
    w.close_box();
    assert!(!kept.tick());
    assert_eq!(w.kinds(), ["machine_detached"]);
    assert!(w.store.box_machine(w.id).unwrap().is_none());
}

#[test]
fn a_parked_box_frees_its_machine_slot() {
    let w = World::new();
    hold_machine(&w);
    let kept = w.kept();
    assert!(kept.tick());
    kept.park();
    assert_eq!(w.kinds(), ["machine_detached"]);
    assert!(w.store.box_machine(w.id).unwrap().is_none());
}

#[test]
fn a_park_for_hands_the_box_replaced_keeps_the_slot_and_logs_the_stale_write() {
    let w = World::new();
    hold_machine(&w);
    let kept = w.kept();
    assert!(kept.tick());
    w.store.replace_hands(w.id);
    kept.park();
    assert!(w.kinds().is_empty());
    assert!(w.store.box_machine(w.id).unwrap().is_some(), "still held");
    assert!(String::from_utf8_lossy(&w.log.lock().unwrap()).contains("stale_hands_write"));
}

mod egress;
mod host;
