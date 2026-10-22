//! The client against a real controller: a watch resumes across a SIGTERM drain and restart
//! under a socket a holder keeps open (as systemd does), and `moor show` fails fast when no
//! controller listens.

#![allow(
    clippy::disallowed_methods,
    reason = "the tests time a real binary with the wall clock"
)]

use std::fs::{self, File};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use command_fds::{CommandFdExt as _, FdMapping};
use moor_client::{Client, Redial, WatchItem};
use moor_core::controller::WatchFilter;
use moor_core::request::Request;

use super::fresh_dir;

struct Held {
    root: PathBuf,
    config: PathBuf,
    socket: PathBuf,
    state: PathBuf,
    listener: OwnedFd,
}

impl Held {
    fn new() -> Self {
        let root = fresh_dir("redial-e2e");
        let socket = root.join("c.sock");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        let config = root.join("c.yaml");
        fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
                socket.display(),
                state.display()
            ),
        )
        .unwrap();
        let l = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            root,
            config,
            socket,
            state,
            listener: l.into(),
        }
    }

    fn serve(&self) -> Child {
        let mut cmd = Command::new("/bin/sh");
        cmd.current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .args(["-c", r#"export LISTEN_PID=$$; exec "$0" "$@""#])
            .arg(env!("CARGO_BIN_EXE_moor"));
        cmd.fd_mappings(vec![FdMapping {
            parent_fd: self.listener.try_clone().unwrap(),
            child_fd: 3,
        }])
        .unwrap();
        cmd.args(["controller", "serve", "--config"])
            .arg(&self.config)
            .env("LISTEN_FDS", "1")
            .env("LISTEN_FDNAMES", "controller")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(self.root.join("stderr")).unwrap())
            .spawn()
            .unwrap()
    }

    fn db(&self) -> rusqlite::Connection {
        let c = rusqlite::Connection::open(self.state.join("moor.db")).unwrap();
        c.busy_timeout(Duration::from_secs(10)).unwrap();
        c
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.root.join("stderr")).unwrap_or_default()
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn add_events(held: &Held, from: usize, to: usize) {
    let db = held.db();
    for i in from..=to {
        db.execute(
            "INSERT INTO events (box, kind, payload, ts) VALUES (1, 'copied', ?1, 0)",
            [format!(r#"{{"from":"a{i}","to":"b{i}"}}"#)],
        )
        .unwrap();
    }
}

fn wait_for(path: &Path) {
    let t = Instant::now();
    while !path.exists() {
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "no {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn term(child: &mut Child) -> Option<i32> {
    let pid = rustix::process::Pid::from_child(child);
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    for _ in 0..1000 {
        if let Some(s) = child.try_wait().unwrap() {
            return s.code();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// Receives watch items until `cursors` holds `n` event cursors.
fn take_events(cursors: &mut Vec<i64>, rx: &mpsc::Receiver<Item>, n: usize) {
    while cursors.len() < n {
        match rx
            .recv_timeout(Duration::from_secs(20))
            .expect("watch item")
        {
            Ok(WatchItem::Event(e)) => cursors.push(e.cursor),
            Ok(WatchItem::CaughtUp { .. }) => {}
            Err(e) => panic!("watch error: {e}"),
        }
    }
}

type Item = Result<WatchItem, moor_client::ClientError>;

#[test]
fn a_watch_resumes_across_a_real_drain_and_restart_with_no_gap_or_duplicate() {
    let held = Held::new();
    let mut a = held.serve();
    let client = Client::new(&held.socket).with_redial(Redial::within(Duration::from_secs(30)));
    client.request(Request::Version {}, |_| {}).unwrap();
    wait_for(&held.state.join("moor.db"));
    held.db()
        .execute(
            "INSERT INTO boxes (name, state, created_at, updated_at) VALUES ('a', 'working', 0, 0)",
            [],
        )
        .unwrap();
    add_events(&held, 1, 3);

    let watch = client.watch(0, WatchFilter::default(), true).unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for item in watch {
            if tx.send(item).is_err() {
                return;
            }
        }
        let _ = tx.send(Err(moor_client::ClientError::Protocol(
            "watch ended".into(),
        )));
    });
    let mut cursors = Vec::new();
    take_events(&mut cursors, &rx, 3);
    assert_eq!(cursors, [1, 2, 3], "{}", held.stderr());

    assert_eq!(term(&mut a), Some(0), "{}", held.stderr());
    add_events(&held, 4, 5);
    let req = std::thread::spawn(move || client.request(Request::Version {}, |_| {}));
    std::thread::sleep(Duration::from_millis(1500));
    let mut b = held.serve();
    take_events(&mut cursors, &rx, 5);
    add_events(&held, 6, 6);
    take_events(&mut cursors, &rx, 6);
    assert_eq!(cursors, [1, 2, 3, 4, 5, 6], "{}", held.stderr());
    req.join()
        .unwrap()
        .expect("a request made while no controller ran is answered once one is back");
    while let Ok(item) = rx.recv_timeout(Duration::from_millis(500)) {
        match item {
            Ok(WatchItem::Event(e)) => panic!("duplicate or extra event {}", e.cursor),
            Ok(WatchItem::CaughtUp { .. }) => {}
            Err(e) => panic!("watch error: {e}"),
        }
    }
    assert_eq!(term(&mut b), Some(0), "{}", held.stderr());
    drop(rx);
    drop(reader);
}

/// `moor --json show` against `socket`, and how long it took.
fn show_via(socket: &Path) -> (Output, Duration) {
    let home = socket.with_file_name("home");
    let began = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("MOOR_CONTROLLER", socket)
        .args(["--json", "show"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out, began.elapsed())
}

/// The error message of a failed `--json` envelope.
fn failure(out: &Output) -> String {
    assert_ne!(out.status.code(), Some(0));
    let envelope: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let message = envelope
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str);
    message.unwrap().to_owned()
}

#[test]
fn show_fails_within_a_second_when_the_socket_path_is_missing() {
    let root = fresh_dir("redial-missing");
    let socket = root.join("c.sock");
    let (out, took) = show_via(&socket);
    let message = failure(&out);
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert_eq!(
        message,
        format!(
            "controller at {} is unreachable: No such file or directory (os error 2)",
            socket.display()
        )
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn show_stops_redialing_a_stale_socket_after_about_five_seconds() {
    let root = fresh_dir("redial-stale");
    let socket = root.join("c.sock");
    drop(UnixListener::bind(&socket).unwrap());
    let (out, took) = show_via(&socket);
    let message = failure(&out);
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_secs(8),
        "{took:?}"
    );
    assert!(
        message.contains("not back after 5s") && message.contains("refused"),
        "{message}"
    );
    let _ = fs::remove_dir_all(&root);
}
