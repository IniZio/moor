//! A Box's keeper against the store and a fake driver: its tunnel follows the Box's handle, is
//! reopened when it dies, is parked while the Box sleeps, and goes with the Box.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
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
use moor_core::request::{MachineLink, RunnerAddr};
use moor_core::seam::clock::Clock as _;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::user::Identity;
use moor_core::{BoxHandle, BoxId, BoxName, HoldOp, ProviderId};

use super::{Deps, Kept};
use crate::proxy::links::Runners;
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
        store.reserve(id, &FakeRng::default()).unwrap();
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
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
        let runners = Runners::new(self.dir.path(), None, clock, None);
        self.kept_via(tunnels, runners)
    }

    /// A keeper that reaches machines through `runners`.
    fn kept_via(&self, tunnels: bool, runners: Runners) -> Kept {
        let deps = Deps {
            store: self.store.clone(),
            driver: Arc::clone(&self.driver) as Arc<dyn Driver>,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            egress: tunnels.then(|| self.socket()),
            runners,
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
pub(crate) fn ready_box(store: &Store, driver: &FakeDriver) -> BoxId {
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

/// A machine runner across the network: answers each forward request `up` and holds it; its
/// address and, per connection, the hello it opened with.
fn tcp_runner() -> (String, Receiver<(String, TcpStream)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, asked) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = conn.unwrap();
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let (mut hello, mut ask) = (String::new(), String::new());
            reader.read_line(&mut hello).unwrap();
            reader.read_line(&mut ask).unwrap();
            conn.write_all(b"{\"reply\":\"up\"}\n").unwrap();
            drop(tx.send((hello, conn)));
        }
    });
    (addr, asked)
}

/// Records that the runner at `addr` holds machine `machine_id`'s link at `clock`'s now.
fn link_row(w: &World, clock: &FakeClock, addr: &str, machine_id: i64) {
    let link = MachineLink {
        machine_id,
        online: true,
        since: clock.now(),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        warm_starts: None,
        memory: None,
    };
    let runner = RunnerAddr::new(addr);
    w.store
        .set_machine_links(&runner, false, &[link], clock.now())
        .unwrap();
}

#[test]
fn forwards_reach_the_runner_the_link_row_names_and_reopen_when_it_moves() {
    let w = World::new([]);
    hold_machine(&w);
    let machine_id = w.store.box_machine(w.id).unwrap().unwrap().machine_id;
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let runners = Runners::new(
        w.dir.path(),
        Some(w.store.clone()),
        clock.clone(),
        Some("k"),
    );
    let kept = w.kept_via(false, runners);
    let (first, at_first) = tcp_runner();
    link_row(&w, &clock, &first, machine_id);
    assert!(kept.tick());
    assert_eq!(w.kinds(), ["machine_attached"]);
    let (hello, _held) = at_first.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(hello, "{\"forwards\":{\"v\":1,\"key\":\"k\"}}\n");
    assert!(kept.tick());
    assert_eq!(at_first.try_iter().count(), 0, "live forwards are kept");

    clock.sleep(Duration::from_secs(1));
    let (second, at_second) = tcp_runner();
    link_row(&w, &clock, &second, machine_id);
    assert!(kept.tick());
    let reopened = at_second.recv_timeout(Duration::from_secs(5));
    assert!(
        reopened.is_ok(),
        "the forwards reopen on the runner the row names now"
    );
    assert_eq!(w.kinds(), ["machine_attached", "machine_attached"]);
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

/// Holds each clear of the Box port until released; says when one began.
struct Gate {
    began: Receiver<()>,
    release: mpsc::Sender<()>,
}

fn gate_clears(driver: &FakeDriver) -> Gate {
    let (began_tx, began) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let (began_tx, release_rx) = (Mutex::new(began_tx), Mutex::new(release_rx));
    driver.on_exec_of("sshd", move || {
        began_tx.lock().unwrap().send(()).ok();
        release_rx.lock().unwrap().recv().ok();
    });
    Gate { began, release }
}

#[test]
fn up_and_park_answer_while_the_tunnel_opens() {
    let w = World::new([]);
    let gate = gate_clears(&w.driver);
    let kept = Arc::new(w.kept(true));
    let ticking = Arc::clone(&kept);
    let tick = std::thread::spawn(move || ticking.tick());
    gate.began.recv().unwrap();
    let (done_tx, done) = mpsc::channel();
    let asking = Arc::clone(&kept);
    std::thread::spawn(move || {
        let up = asking.up();
        asking.park();
        done_tx.send(up).ok();
    });
    let answered = done.recv_timeout(Duration::from_secs(5));
    gate.release.send(()).unwrap();
    assert_eq!(answered, Ok(false), "up and park did not wait for the open");
    assert!(tick.join().unwrap());
    assert!(!kept.up(), "a tunnel opened for a parked Box is not kept");
    assert_eq!(
        w.driver.tunnels().len(),
        w.driver.dropped_tunnels().len(),
        "every tunnel opened was closed"
    );
}

#[test]
fn an_ended_keeper_finishes_its_open_before_end_returns_and_keeps_nothing() {
    let w = World::new([]);
    let gate = gate_clears(&w.driver);
    let keepers: &'static super::Keepers = Box::leak(Box::default());
    let kept = Arc::new(w.kept(true));
    super::lock(&keepers.kept).insert(w.id, Arc::clone(&kept));
    let ticking = Arc::clone(&kept);
    let tick = std::thread::spawn(move || ticking.tick());
    gate.began.recv().unwrap();
    let (ended_tx, ended) = mpsc::channel();
    let id = w.id;
    std::thread::spawn(move || {
        drop(keepers.end(id));
        ended_tx.send(()).ok();
    });
    assert!(
        ended.recv_timeout(Duration::from_millis(300)).is_err(),
        "end waits for the open in flight"
    );
    gate.release.send(()).unwrap();
    ended.recv_timeout(Duration::from_secs(5)).unwrap();
    tick.join().unwrap();
    drop(gate);
    assert_eq!(
        w.driver.tunnels().len(),
        w.driver.dropped_tunnels().len(),
        "the ended keeper keeps no tunnel"
    );
    assert!(!kept.tick(), "an ended keeper does not tick again");
    assert_eq!(w.driver.tunnels().len(), 1, "and opens nothing more");
}

/// The controller's Proxy host over `w`, delivering a stand-in forwarder for the direct route.
fn kept_proxy(w: &World) -> super::KeptProxy {
    let binary = w.dir.path().join("moor-egress-forward");
    std::fs::write(&binary, b"forwarder-bytes").unwrap();
    let deps = Deps {
        store: w.store.clone(),
        driver: Arc::clone(&w.driver) as Arc<dyn Driver>,
        clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
        egress: None,
        runners: Runners::new(
            w.dir.path(),
            None,
            Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            None,
        ),
        log: w.log.clone(),
    };
    super::KeptProxy {
        deps: Arc::new(deps),
        rng: Arc::new(FakeRng::default()),
        vault_key: Some(crate::testing::cx::test_vault_key(w.dir.path())),
        state_dir: None,
        keepers: Box::leak(Box::<super::Keepers>::default()),
        route: crate::proxy::route::EgressRoute::parse("wss://egress.test/egress").unwrap(),
        key: zeroize::Zeroizing::new(b"egress-key".to_vec()),
        binary,
    }
}

#[test]
fn a_row_from_before_the_forwarder_gets_a_forward_port_and_the_exec_delivers_it() {
    use moor_core::seam::proxy_host::{ProxyHost as _, ProxySpec};
    use moor_core::BoxPath;
    use moor_schema::tables::Proxy;
    use sea_query::{Expr, ExprTrait as _, Query};

    use crate::store::db::DbExt as _;

    let w = World::new([]);
    let before = w.store.proxy(w.id).unwrap().unwrap();
    w.store
        .writer()
        .run(
            Query::update()
                .table(Proxy::Table)
                .value(Proxy::ForwardPort, Option::<i64>::None)
                .and_where(Expr::col(Proxy::Box).eq(w.id.get())),
        )
        .unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    w.driver
        .put_file(&w.handle(), &environ, b"HOME=/home/box\0", 0o400)
        .unwrap();
    let host = kept_proxy(&w);
    let spec = ProxySpec {
        box_id: w.id,
        box_handle: Some(w.handle()),
        box_repo: None,
        other_repos: Vec::new(),
        rules: Vec::new(),
        user: moor_core::user::UserId::new(1),
    };
    let ep = host.ensure(&spec).unwrap();
    let after = w.store.proxy(w.id).unwrap().unwrap();
    assert!(
        after.forward_port.is_some(),
        "the old row got a forward port"
    );
    assert_eq!(ep.forward_port, after.forward_port);
    assert_eq!(
        (after.box_port, after.egress_port),
        (before.box_port, before.egress_port),
        "its other ports are kept"
    );
    let delivered: Vec<String> = w
        .driver
        .puts()
        .into_iter()
        .map(|(_, path, _, _)| path.as_str().to_owned())
        .collect();
    assert!(
        delivered.contains(&"/home/box/.moor/egress-forward.json".to_owned()),
        "{delivered:?}"
    );
}
