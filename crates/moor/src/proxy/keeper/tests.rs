//! A Box's keeper against the store and a fake driver: its tunnel follows the Box's handle, is
//! reopened when it dies, is parked while the Box sleeps, and goes with the Box.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::user::Identity;
use moor_core::{BoxHandle, BoxId, BoxName, HoldOp, ProviderId};

use super::{Deps, Kept};
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::{FakeDriver, TunnelReply};
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
    fn new(replies: impl IntoIterator<Item = TunnelReply>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let driver = Arc::new(FakeDriver::new().reverse_tunnels().tunnel_replies(replies));
        let id = ready_box(&store, &driver);
        drop(
            store
                .reserve(id, &FakeRng::default(), Duration::ZERO)
                .unwrap(),
        );
        Self {
            dir,
            store,
            driver,
            id,
            log: Arc::default(),
        }
    }

    /// A keeper that tunnels to the shared Proxy's socket when `tunnels`.
    fn kept(&self, tunnels: bool) -> Kept {
        let deps = Deps {
            store: self.store.clone(),
            driver: Arc::clone(&self.driver) as Arc<dyn Driver>,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            egress: tunnels.then(|| self.socket()),
            runner: crate::runner_dir::of(self.dir.path()).join(moor_client::forward::SOCKET),
            log: self.log.clone(),
        };
        Kept::new(Arc::new(deps), self.id, 41_000)
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("egress.sock")
    }

    fn handle(&self) -> BoxHandle {
        let row = self.store.box_row(self.id).unwrap().unwrap();
        row.boxed.unwrap().handle.unwrap()
    }

    /// The handles each tunnel was opened into, in order.
    fn opened(&self) -> Vec<String> {
        let calls = self.driver.tunnels().into_iter();
        calls.map(|c| c.handle.as_str().to_owned()).collect()
    }

    /// Gives the Box a fresh machine, as a retry that moves it does.
    fn move_box(&self) -> BoxHandle {
        for input in [BoxInput::DriverMissing, BoxInput::Requested] {
            self.store
                .apply(Decision::new(self.id).box_input(input))
                .unwrap();
        }
        let handle = acquire(&self.driver, self.id, "moved");
        let acquired = BoxInput::Acquired {
            handle: handle.clone(),
        };
        self.store
            .apply(Decision::new(self.id).box_input(acquired))
            .unwrap();
        handle
    }

    fn close_box(&self) {
        let d = Decision::new(self.id)
            .lifecycle(BoxLifecycleInput::Close { reason: None })
            .session(SessionInput::End)
            .box_input(BoxInput::Released);
        self.store.apply(d).unwrap();
    }

    fn kinds(&self) -> Vec<String> {
        let events = self.store.events(self.id, 50).unwrap().into_iter();
        events
            .map(|e| e.kind)
            .filter(|k| k.starts_with("machine_") || k == "tunnel_restarted")
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
fn ready_box(store: &Store, driver: &FakeDriver) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(new)).unwrap().box_id;
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(admitted).unwrap();
    let handle = acquire(driver, id, "demo");
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle });
    store.apply(ready).unwrap();
    id
}

#[test]
fn the_first_tick_opens_the_tunnel_from_the_box_port_to_the_shared_proxy() {
    let w = World::new([]);
    let kept = w.kept(true);
    assert!(kept.tick());
    let calls = w.driver.tunnels();
    let [call] = calls.as_slice() else {
        unreachable!("{calls:?}")
    };
    assert_eq!(
        (&call.handle, call.box_port, &call.socket),
        (&w.handle(), 41_000, &w.socket())
    );
    assert!(kept.up());
    assert!(kept.tick());
    assert_eq!(w.driver.tunnels().len(), 1, "a live tunnel is kept");
}

#[test]
fn a_box_that_dials_the_proxy_itself_gets_no_tunnel() {
    let w = World::new([]);
    let kept = w.kept(false);
    assert!(kept.tick());
    assert!(w.driver.tunnels().is_empty());
    assert!(kept.up());
}

#[test]
fn a_box_moved_to_a_fresh_machine_gets_its_tunnel_there_on_the_next_tick() {
    let w = World::new([]);
    let kept = w.kept(true);
    let old = w.handle();
    assert!(kept.tick());
    let moved = w.move_box();
    assert!(kept.tick());
    assert_eq!(w.opened(), [old.as_str(), moved.as_str()]);
    assert_eq!(
        w.driver.dropped_tunnels().len(),
        1,
        "the old tunnel is closed"
    );
    assert!(kept.up());
}

#[test]
fn a_dead_tunnel_is_opened_again_and_recorded() {
    let w = World::new([TunnelReply::ExitAfterReady]);
    let kept = w.kept(true);
    assert!(kept.tick());
    assert!(!kept.up());
    assert!(kept.tick());
    assert_eq!(w.driver.tunnels().len(), 2);
    assert!(kept.up());
    assert_eq!(w.kinds(), ["tunnel_restarted"]);
}

#[test]
fn a_failed_open_waits_before_it_is_tried_again() {
    let hang = TunnelReply::EarlyExit {
        code: 255,
        stderr: "ssh: connect refused".into(),
    };
    let w = World::new([hang.clone(), hang]);
    let kept = w.kept(true);
    assert!(kept.tick());
    assert!(kept.tick());
    assert_eq!(w.driver.tunnels().len(), 1, "the second try waits");
    assert!(kept.bring_up().is_err(), "a direct call tries at once");
    assert!(kept.bring_up().is_ok());
    assert_eq!(w.driver.tunnels().len(), 3);
}

#[test]
fn a_parked_box_keeps_no_tunnel_until_it_is_restored() {
    let w = World::new([]);
    let kept = w.kept(true);
    assert!(kept.tick());
    kept.park();
    assert_eq!(w.driver.dropped_tunnels().len(), 1);
    assert!(kept.tick());
    assert_eq!(w.driver.tunnels().len(), 1, "no tunnel while parked");
    kept.restore().unwrap();
    assert_eq!(w.driver.tunnels().len(), 2);
    assert!(kept.up());
}

#[test]
fn a_closed_box_ends_its_keeper_and_loses_its_proxy_row() {
    let w = World::new([]);
    let kept = w.kept(true);
    assert!(kept.tick());
    w.close_box();
    assert!(!kept.tick());
    assert_eq!(w.driver.dropped_tunnels().len(), 1);
    assert!(w.store.proxy(w.id).unwrap().is_none());
}

/// What the machine runner was asked.
type Asked = Receiver<UnixStream>;

/// Stands in for the machine runner's forward socket: answers each request `up` and holds it.
fn runner(w: &World) -> Asked {
    let dir = crate::runner_dir::of(w.dir.path());
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join(moor_client::forward::SOCKET)).unwrap();
    let (tx, asked) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = conn.unwrap();
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            conn.write_all(b"{\"reply\":\"up\"}\n").unwrap();
            drop(tx.send(conn));
        }
    });
    asked
}

/// The Box's requester holds the machine `mini`, forwarding Box port 8081.
fn hold_machine(w: &World) {
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
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
fn the_machine_is_attached_reattached_on_a_new_machine_and_let_go_on_close() {
    let w = World::new([]);
    hold_machine(&w);
    let asked = runner(&w);
    let kept = w.kept(false);
    assert!(kept.tick());
    assert_eq!(w.kinds(), ["machine_attached"]);
    assert!(kept.tick());
    assert_eq!(asked.try_iter().count(), 1, "live forwards are kept");
    w.move_box();
    assert!(kept.tick());
    assert_eq!(w.kinds(), ["machine_attached", "machine_attached"]);
    w.close_box();
    assert!(!kept.tick());
    assert_eq!(
        w.kinds(),
        ["machine_attached", "machine_attached", "machine_detached"]
    );
    assert!(w.store.box_machine(w.id).unwrap().is_none());
}

#[test]
fn a_parked_box_frees_its_machine_slot() {
    let w = World::new([]);
    hold_machine(&w);
    let _asked = runner(&w);
    let kept = w.kept(false);
    assert!(kept.tick());
    kept.park();
    assert_eq!(w.kinds(), ["machine_attached", "machine_detached"]);
    assert!(w.store.box_machine(w.id).unwrap().is_none());
}
