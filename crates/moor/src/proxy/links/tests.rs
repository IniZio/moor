#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use moor_client::link::broker::{Ask, Brokered, PoolOp};
use moor_client::link::ops::{self, Reply};
use tokio::net::UnixListener;

use super::*;

fn links(state_dir: &Path) -> RunnerLinks {
    RunnerLinks {
        state_dir: state_dir.to_owned(),
        socket: runner_dir::of(state_dir).join(broker::SOCKET),
        moor: PathBuf::from("/bin/moor"),
    }
}

fn sample() -> Status {
    Status {
        version: "1.2.3".into(),
        protocol: 2,
        os: "linux".into(),
        arch: "x86_64".into(),
        free_disk_gb: Some(40),
        pool: None,
        recent: Vec::new(),
    }
}

/// Serves one broker request on the runner socket under `state_dir` with `reply` and returns
/// the request it saw.
fn broker_once(state_dir: &Path, reply: Brokered) -> std::thread::JoinHandle<Ask> {
    let dir = runner_dir::of(state_dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(dir.join(broker::SOCKET)).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = UnixListener::from_std(listener).unwrap();
                let (mut conn, _) = listener.accept().await.unwrap();
                let ask: Ask = ops::read_line(&mut conn).await.unwrap();
                ops::write_line(&mut conn, &reply).await.unwrap();
                ask
            })
    })
}

#[test]
fn the_dial_command_quotes_paths_for_the_shell_and_ssh() {
    let links = links(Path::new("/s/it's 100%"));
    assert_eq!(
        links.dial_command(42, "moor-7"),
        "env MOOR_STATE_DIR='/s/it'\\''s 100%%' '/bin/moor' link-dial 42 moor-7"
    );
}

#[test]
fn a_missing_broker_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let links = links(dir.path());
    assert!(links.lease(1, Verb::List, None, None, None).is_err());
    assert!(links.status(1).is_err());
}

#[test]
fn status_asks_the_broker_for_the_machine() {
    let dir = tempfile::tempdir().unwrap();
    let served = broker_once(
        dir.path(),
        Brokered::Agent {
            reply: Reply::Status(Box::new(sample())),
        },
    );
    let got = links(dir.path()).status(7).unwrap();
    assert_eq!(got, sample());
    let ask = served.join().unwrap();
    assert_eq!(ask.machine_id, 7);
    assert_eq!(ask.op, PoolOp::Status);
}
