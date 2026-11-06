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
    store
        .set_machine_links(&runner, false, &[link], clock.now())
        .unwrap();
}

fn runners(dir: &Path, store: Option<Store>, clock: Arc<FakeClock>) -> Runners {
    let mut runners = Runners::new(dir, store, clock, Some(KEY));
    runners.moor = PathBuf::from("/bin/moor");
    runners
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
        reply: Reply::Leased(Answer::Done),
    });
    link(&store, &clock, &addr, machine_id);
    let runners = runners(dir.path(), Some(store.clone()), Arc::clone(&clock));
    let answer = runners.lease(
        machine_id,
        Verb::Warmup,
        Some("moor-7"),
        None,
        &Boot::default(),
    );
    assert_eq!(answer, Ok(Answer::Done));
    let (hello, ask) = served.join().unwrap();
    assert_eq!(
        hello,
        Hello::Links(Keyed {
            v: 1,
            key: KEY.to_owned()
        })
    );
    assert_eq!(ask.machine_id, machine_id);

    let (addr, served) = remote_once(Brokered::Agent {
        reply: Reply::Status(Box::new(sample())),
    });
    clock.sleep(Duration::from_secs(1));
    link(&store, &clock, &addr, machine_id);
    assert_eq!(runners.status(machine_id), Ok(sample()));
    assert_eq!(served.join().unwrap().1.op, PoolOp::Status);
    assert_eq!(
        runners.dial_command(machine_id, "moor-7"),
        Ok(format!(
            "'/bin/moor' link-dial --runner '{addr}' {machine_id} moor-7"
        ))
    );
}

#[test]
fn a_checkpoint_act_goes_to_the_runner_its_link_row_names() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock, machine_id) = enrolled(dir.path());
    let disk = Answer::Disk {
        free_mib: 7,
        total_mib: 70,
    };
    let (addr, served) = remote_once(Brokered::Agent {
        reply: Reply::Leased(disk.clone()),
    });
    link(&store, &clock, &addr, machine_id);
    let hosts = runners(dir.path(), Some(store), clock);
    let hosts: &dyn crate::adapters::crabbox::PoolHosts = &hosts;
    let got = hosts
        .checkpoint(machine_id, CheckpointAct::Disk {})
        .unwrap();
    assert_eq!(got, disk);
    let (_, ask) = served.join().unwrap();
    let want = PoolOp::Checkpoint {
        act: CheckpointAct::Disk {},
    };
    assert_eq!((ask.machine_id, ask.op), (machine_id, want));
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
    let lease = runners.lease(machine_id, Verb::List, None, None, &Boot::default());
    assert_eq!(lease.unwrap_err(), not_linked);
    let command = runners.dial_command(machine_id, "moor-7");
    assert_eq!(command.unwrap_err(), not_linked);
}

#[test]
fn the_dial_command_quotes_the_address_for_the_shell_and_ssh_and_names_no_state_dir() {
    let dir = Path::new("/s/it's 100%");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let command = runners(dir, None, clock)
        .dial_command(42, "moor-7")
        .unwrap();
    assert_eq!(
        command,
        "'/bin/moor' link-dial --runner '/s/it'\\''s 100%%/runner/runner.sock' 42 moor-7"
    );
    assert!(!command.contains("MOOR_STATE_DIR"), "{command}");
}

#[test]
fn a_missing_broker_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let runners = runners(dir.path(), None, clock);
    assert!(runners
        .lease(1, Verb::List, None, None, &Boot::default())
        .is_err());
    assert!(runners.status(1).is_err());
}

#[test]
fn link_dial_dials_the_runner_it_names_with_the_key_and_the_deadline_ssh_handed_it() {
    let (addr, served) = remote_once(Brokered::Agent {
        reply: Reply::Refused {
            reason: "no VM".into(),
        },
    });
    let env =
        crate::env::Env::from_pairs([("MOOR_MACHINE_SERVICE_KEY", KEY), ("MOOR_DIAL_WITHIN", "7")]);
    let mut stderr = Vec::new();
    let code = crate::link_dial::link_dial(
        &env,
        (&addr, 3, "moor-7"),
        std::io::empty(),
        Vec::new(),
        &mut stderr,
    );
    assert_eq!(code, 1, "{}", String::from_utf8_lossy(&stderr));
    assert!(String::from_utf8_lossy(&stderr).contains("no VM"));
    let (hello, ask) = served.join().unwrap();
    assert!(matches!(hello, Hello::Links(Keyed { key, .. }) if key == KEY));
    assert_eq!(
        ask.op,
        PoolOp::Dial {
            slug: "moor-7".into(),
            timeout_secs: Some(7),
        }
    );
}

#[test]
fn link_dial_to_a_runner_across_the_network_needs_the_key() {
    let env = crate::env::Env::from_pairs([("MOOR_DIAL_WITHIN", "7")]);
    let mut stderr = Vec::new();
    let code = crate::link_dial::link_dial(
        &env,
        ("127.0.0.1:1", 3, "moor-7"),
        std::io::empty(),
        Vec::new(),
        &mut stderr,
    );
    assert_eq!(code, 1);
    let err = String::from_utf8_lossy(&stderr);
    assert!(err.contains("MOOR_MACHINE_SERVICE_KEY"), "{err}");
}
