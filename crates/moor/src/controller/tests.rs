use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use super::{serve, start};
use crate::env::Env;

fn env(dir: &Path) -> Env {
    env_with(dir, &[])
}

/// [`env`] plus the variables `extra`.
fn env_with(dir: &Path, extra: &[(&str, &str)]) -> Env {
    let state = dir.join("state");
    let base = [
        ("MOOR_STATE_DIR", state.to_str().expect("utf8")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ];
    Env::from_pairs(base.iter().chain(extra).copied())
}

fn config(dir: &Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("controller.yaml");
    std::fs::write(&path, text).expect("write config");
    path
}

#[derive(Clone, Default)]
struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("lock").clone()).expect("utf8")
    }
}

#[test]
fn socket_is_owner_only() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let running = start(&env(tmp.path()), &cfg).expect("start");
    assert_eq!(
        running.socket,
        tmp.path().join("state").join("controller.sock")
    );
    let mode = std::fs::metadata(&running.socket)
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn second_instance_names_first() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let first = start(&env(tmp.path()), &cfg).expect("start");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &cfg, out.clone());
    assert_eq!(code, 1);
    let want = format!(
        "already running (pid {}) on {}",
        std::process::id(),
        first.socket.display()
    );
    assert!(out.text().contains(&want), "{}", out.text());
}

#[test]
fn a_lock_held_briefly_by_another_fd_is_waited_out() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("controller.lock");
    let other = std::fs::File::create(&path).expect("create");
    other.lock().expect("held");
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(other);
    });
    let socket = tmp.path().join("controller.sock");
    let taken = super::lock::take_lock(&path, &socket, super::lock::LOCK_WAIT);
    release.join().expect("released");
    assert!(taken.is_ok(), "{:?}", taken.err());
    let recorded = std::fs::read_to_string(&path).expect("read");
    assert_eq!(
        recorded,
        format!("{}\n{}\n", std::process::id(), socket.display())
    );
}

#[test]
fn a_lock_held_past_the_wait_is_a_running_controller_and_a_failure() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("controller.lock");
    let socket = tmp.path().join("controller.sock");
    let short = std::time::Duration::from_millis(100);
    let other = std::fs::File::create(&path).expect("create");
    other.lock().expect("held");
    let stop = super::lock::take_lock(&path, &socket, short).expect_err("held");
    assert_eq!(stop.code, 1);
    assert_eq!(
        stop.message,
        format!("already running: {} is held", path.display())
    );
    std::fs::write(&path, "4242\n/elsewhere.sock\n").expect("record");
    let stop = super::lock::take_lock(&path, &socket, short).expect_err("held");
    assert_eq!(
        (stop.code, stop.message.as_str()),
        (1, "already running (pid 4242) on /elsewhere.sock")
    );
}

#[test]
fn unknown_key_is_usage_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "bogus_key: 1\n");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &cfg, out.clone());
    assert_eq!(code, 2);
    assert!(out.text().contains("bogus_key"), "{}", out.text());
}

#[test]
fn missing_config_names_path() {
    let tmp = tempfile::tempdir().expect("tmp");
    let out = Buf::default();
    let code = serve(&env(tmp.path()), &tmp.path().join("nope.yaml"), out.clone());
    assert_eq!(code, 2);
    assert!(out.text().contains("nope.yaml"), "{}", out.text());
}

#[test]
fn client_gets_a_reply_to_hello() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let running = start(&env(tmp.path()), &cfg).expect("start");
    let socket = running.socket.clone();
    let sink: super::Sink = Arc::new(std::sync::Mutex::new(Buf::default()));
    running.serve_in_background(sink);
    let mut stream = UnixStream::connect(&socket).expect("connect");
    stream
        .write_all(b"{\"frame\":\"hello\",\"protocol\":1}\n")
        .expect("send");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("read");
    assert!(!line.is_empty());
}

#[test]
fn a_repos_key_stops_the_controller_as_an_unknown_key() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(
        tmp.path(),
        "repos:
  app:
    url: https://github.com/o/app.git
",
    );
    let stop = start(&env(tmp.path()), &cfg).err().expect("stop");
    assert!(
        stop.message.contains("unknown field `repos`"),
        "{}",
        stop.message
    );
}

mod artifacts;
mod authz;
mod front_attach;
mod front_processes;
mod front_proxy;
mod front_read_only;
mod fronts;
mod host_paths;
mod host_workdirs;
mod ledger_socket;
mod ledger_tls;
mod machine_hold;
mod operator_only;
mod run_cancel;
mod run_grant;
mod run_output;
mod runner_socket;
mod serve_first;
mod socket_path_len;
mod users;
mod webhook_front;

#[test]
fn first_start_creates_the_vault_key_beside_the_config() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    drop(start(&env(tmp.path()), &cfg).expect("start"));
    let key = tmp.path().join("vault.key");
    let mode = std::fs::metadata(&key)
        .expect("vault.key")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!tmp.path().join("state").join("vault.key").exists());
}

#[test]
fn a_vault_key_that_is_not_owner_only_stops_the_start() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "");
    let key = tmp.path().join("vault.key");
    std::fs::write(&key, "x\n").expect("key");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let stop = start(&env(tmp.path()), &cfg).err().expect("refused");
    assert_eq!(stop.code, 3);
    assert!(stop.message.contains("it must be 0600"), "{}", stop.message);
    assert_eq!(std::fs::read_to_string(&key).expect("kept"), "x\n");
}

#[test]
fn a_config_inside_the_state_dir_is_refused_for_the_vault_key() {
    let tmp = tempfile::tempdir().expect("tmp");
    let state = tmp.path().join("state");
    std::fs::create_dir(&state).expect("state");
    let cfg = config(&state, "");
    let stop = start(&env(tmp.path()), &cfg).err().expect("refused");
    assert_eq!(stop.code, 3);
    assert!(
        stop.message.contains("inside the state dir"),
        "{}",
        stop.message
    );
    assert!(!state.join("vault.key").exists());
}

#[test]
fn the_configured_drain_timeout_bounds_the_live_drain() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = config(tmp.path(), "drain_timeout_secs: 1\n");
    let running = start(&env(tmp.path()), &cfg).expect("start");
    let drain = Arc::new(super::Drain::default());
    let _in_flight = drain.hold().expect("held");
    let (wake, signal) = UnixStream::pair().expect("pair");
    let out = Buf::default();
    let sink: super::Sink = Arc::new(std::sync::Mutex::new(out.clone()));
    let (done, returned) = std::sync::mpsc::channel();
    let serving = Arc::clone(&drain);
    std::thread::spawn(move || {
        running.serve_until(&wake, &serving, &sink);
        done.send(()).ok();
    });
    (&signal).write_all(&[1]).expect("wake");
    assert!(
        returned
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err(),
        "the drain returned before drain_timeout_secs"
    );
    returned
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the drain outlived drain_timeout_secs");
    assert!(out.text().contains("exiting with 1 "), "{}", out.text());
}
