//! The controller's own egress key.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;

use super::{keep, key, prepare_egress, KEY_FILE};
use crate::env::{Env, EGRESS_KEY};

#[test]
fn the_key_is_made_once_owner_only_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let first = key(dir.path()).unwrap();
    assert!(first.len() >= 40, "{first}");
    let mode = std::fs::metadata(dir.path().join(KEY_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(key(dir.path()).unwrap(), first);
}

#[test]
fn an_empty_key_file_left_by_a_crash_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(KEY_FILE), "").unwrap();
    let made = key(dir.path()).unwrap();
    assert!(made.len() >= 40, "{made}");
    assert_eq!(key(dir.path()).unwrap(), made);
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1, "no draft is left behind");
}

#[test]
fn a_proxy_elsewhere_needs_the_key_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().to_str().unwrap();
    let env = |extra: &[(&str, &str)]| {
        let base = [
            ("MOOR_STATE_DIR", state),
            ("MOOR_EGRESS_URL", "wss://egress.test/egress"),
        ];
        Env::from_pairs(base.iter().chain(extra).copied())
    };
    let err = prepare_egress(env(&[])).err().unwrap();
    assert!(err.contains(EGRESS_KEY), "{err}");
    let kept = prepare_egress(env(&[(EGRESS_KEY, "given")])).unwrap();
    assert_eq!(kept.get(EGRESS_KEY), Some("given"));
    assert!(!dir.path().join(KEY_FILE).exists(), "no key of its own");
}

/// Names the socket and key id [`answer_ready_as`] serves in a child process.
const READY_AS: &str = "MOOR_TEST_READY_AS";

#[test]
fn answer_ready_as() {
    let Some(spec) = std::env::var_os(READY_AS) else {
        return;
    };
    let spec = spec.into_string().unwrap();
    let (socket, id) = spec.rsplit_once(':').unwrap();
    let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
    for mut conn in listener.incoming().flatten() {
        let mut head = [0_u8; 256];
        drop(conn.read(&mut head));
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{id}",
            id.len()
        );
        drop(conn.write_all(reply.as_bytes()));
    }
}

#[test]
fn a_proxy_under_another_key_is_stopped_and_one_under_the_same_key_kept() {
    if std::env::var_os(READY_AS).is_some() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("egress.sock");
    let mut child = moor_testkit::ChildGuard::new(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "wire::egress::tests::answer_ready_as"])
            .env(READY_AS, format!("{}:7", socket.display()))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let listening = (0..500).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::os::unix::net::UnixStream::connect(&socket).is_ok()
    });
    assert!(listening, "the stand-in Proxy listens");
    assert_eq!(keep(&socket, 7), Ok(true), "the same key is kept");
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(keep(&socket, 8), Ok(false), "another key is replaced");
    let status = child.wait().unwrap();
    assert!(!status.success(), "it was stopped: {status}");
    assert_eq!(keep(&socket, 8), Ok(false), "nothing listens now");
}
