#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::time::Duration;

use moor_client::link::broker::{Ask, Brokered, PoolOp};
use moor_client::link::ops::{self, Reply};
use moor_client::link::reach::{Hello, Keyed};
use moor_core::config::EnrolledMachine;
use moor_core::request::{MachineLink, RunnerAddr};
use moor_core::seam::clock::Ts;
use moor_core::user::Identity;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, UnixListener};

use super::*;
use crate::testing::clock::FakeClock;

const KEY: &str = "machine-service-key";

fn sample() -> Status {
    Status {
        version: "1.2.3".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        free_disk_gb: Some(40),
        pool: None,
        recent: Vec::new(),
    }
}

/// Runs `serve` on a runtime of its own on a thread; its result once it ends.
fn on_thread<T: Send + 'static>(
    serve: impl Future<Output = T> + Send + 'static,
) -> std::thread::JoinHandle<T> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(serve)
    })
}

/// Reads one ask on `conn` and answers it with `reply`; the ask.
async fn answer<S: AsyncRead + AsyncWrite + Unpin + Send>(mut conn: S, reply: &Brokered) -> Ask {
    let ask: Ask = ops::read_line(&mut conn).await.unwrap();
    ops::write_line(&mut conn, reply).await.unwrap();
    ask
}

/// Serves one broker request on the runner socket under `state_dir` with `reply`; the ask.
fn local_once(state_dir: &Path, reply: Brokered) -> std::thread::JoinHandle<Ask> {
    let dir = runner_dir::of(state_dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(dir.join(broker::SOCKET)).unwrap();
    listener.set_nonblocking(true).unwrap();
    on_thread(async move {
        let listener = UnixListener::from_std(listener).unwrap();
        let (conn, _) = listener.accept().await.unwrap();
        answer(conn, &reply).await
    })
}

/// A runner across the network that serves one broker request with `reply`; its address, and
/// the hello and ask it saw.
fn remote_once(reply: Brokered) -> (String, std::thread::JoinHandle<(Hello, Ask)>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    listener.set_nonblocking(true).unwrap();
    let served = on_thread(async move {
        let listener = TcpListener::from_std(listener).unwrap();
        let (mut conn, _) = listener.accept().await.unwrap();
        let hello: Hello = ops::read_line(&mut conn).await.unwrap();
        (hello, answer(conn, &reply).await)
    });
    (addr, served)
}

/// A store under `dir` with machine `hv1` enrolled; the store, its clock and the machine's id.
fn enrolled(dir: &Path) -> (Store, Arc<FakeClock>, i64) {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.join("moor.db"), Arc::clone(&clock) as Arc<dyn Clock>).unwrap();
    let (user, _) = store
        .enroll(&Identity::parse("authgear:a").unwrap(), None)
        .unwrap();
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
    let machine = EnrolledMachine::parse("hv1", key, &[], 1).unwrap();
    let machine_id = store.enroll_machine(user, &machine).unwrap();
    (store, clock, machine_id)
}

/// Records that runner `addr` holds machine `machine_id`'s link now.
fn link(store: &Store, clock: &FakeClock, addr: &str, machine_id: i64) {
    let link = MachineLink {
        provides: None,
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
        memory: None,
    };
    let runner = RunnerAddr::new(addr);
    store
        .set_machine_links(&runner, false, &[link], clock.now())
        .unwrap();
}

fn runners(dir: &Path, store: Option<Store>, clock: Arc<FakeClock>) -> Runners {
    Runners::new(dir, store, clock, Some(KEY))
}

#[test]
fn without_a_store_the_runner_on_this_host_answers_for_every_machine() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let served = local_once(
        dir.path(),
        Brokered::Agent {
            reply: Reply::Status(Box::new(sample())),
        },
    );
    let got = runners(dir.path(), None, clock).status(7).unwrap();
    assert_eq!(got, sample());
    let ask = served.join().unwrap();
    assert_eq!((ask.machine_id, ask.op), (7, PoolOp::Status));
}

#[test]
fn a_machine_is_reached_through_the_runner_its_link_row_names_with_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, machine_id) = enrolled(dir.path());
    let other = runner_dir::of(dir.path()).join("runner.sock");
    link(&store, &clock, other.to_str().unwrap(), machine_id + 1);
    let (addr, served) = remote_once(Brokered::Agent {
        reply: Reply::Status(Box::new(sample())),
    });
    link(&store, &clock, &addr, machine_id);
    let runners = runners(dir.path(), Some(store.clone()), Arc::clone(&clock));
    assert_eq!(runners.status(machine_id), Ok(sample()));
    let (hello, ask) = served.join().unwrap();
    assert_eq!(
        hello,
        Hello::Links(Keyed {
            v: 1,
            key: KEY.to_owned()
        })
    );
    assert_eq!(ask.machine_id, machine_id);
}

#[test]
fn a_link_row_not_heard_from_in_a_minute_is_not_linked() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, machine_id) = enrolled(dir.path());
    link(&store, &clock, "10.0.0.7:7701", machine_id);
    clock.sleep(Duration::from_secs(61));
    let runners = runners(dir.path(), Some(store), clock);
    let not_linked = format!("machine {machine_id} is not linked");
    assert_eq!(runners.status(machine_id).unwrap_err(), not_linked);
}

#[test]
fn a_missing_broker_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let runners = runners(dir.path(), None, clock);
    assert!(runners.status(1).is_err());
}
