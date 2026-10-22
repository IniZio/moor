#![allow(
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::panic,
    reason = "tests index fixed fixtures, time the redial with the wall clock and fail loudly"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::controller::WatchFilter;
use moor_core::{BoxName, ErrorKind, ExitClass, Request, WaitReq};

use super::{fake_ssh, hello, ok_envelope, show, ssh_target};
use crate::client::{call, failure_response, watch, ClientError, Env, Target};

/// One connection's far end: reads a line, writes lines; every line read is recorded.
struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Peer {
    /// Answers the hello and returns the request or watch line after it.
    fn take_request(&mut self) -> String {
        let mut hello_line = String::new();
        self.reader.read_line(&mut hello_line).unwrap();
        writeln!(self.writer, "{}", hello()).unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        let line = line.trim_end().to_owned();
        self.lines.lock().unwrap().push(line.clone());
        line
    }

    fn say(&mut self, line: &str) {
        writeln!(self.writer, "{line}").unwrap();
    }
}

type Script = Box<dyn FnOnce(&mut Peer) + Send>;

/// Runs one script per accepted connection; returns the request and watch lines read.
fn controller(socket: &Path, scripts: Vec<Script>) -> Arc<Mutex<Vec<String>>> {
    let listener = UnixListener::bind(socket).unwrap();
    let lines = Arc::<Mutex<Vec<String>>>::default();
    let out = Arc::clone(&lines);
    std::thread::spawn(move || {
        for script in scripts {
            let (stream, _) = listener.accept().unwrap();
            let mut peer = Peer {
                reader: BufReader::new(stream.try_clone().unwrap()),
                writer: stream,
                lines: Arc::clone(&out),
            };
            script(&mut peer);
        }
    });
    lines
}

fn respond_ok() -> Script {
    Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(&format!(
            r#"{{"frame":"response","id":"1","envelope":{}}}"#,
            ok_envelope()
        ));
    })
}

fn redial_env(secs: &str) -> Env {
    Env::from_pairs([("MOOR_CONTROLLER_REDIAL_SECS", secs)])
}

#[test]
fn a_restarting_answer_sends_the_request_again() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let restarting: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    });
    let lines = controller(&sock, vec![restarting, respond_ok()]);
    let mut notes = Vec::new();
    let resp = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &show(),
        |m| notes.push(m.to_owned()),
        |_| {},
    )
    .unwrap();
    assert_eq!(resp.class, ExitClass::Ok);
    assert_eq!(lines.lock().unwrap().len(), 2);
    assert!(
        notes.len() == 1 && notes[0].contains("dialing again"),
        "{notes:?}"
    );
}

#[test]
fn a_session_cut_after_the_request_is_an_error_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let cut: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
    });
    let lines = controller(&sock, vec![cut, respond_ok()]);
    let err = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &show(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("closed the connection")),
        "{err}"
    );
    assert_eq!(lines.lock().unwrap().len(), 1);
}

/// Binds `socket` and closes the listener, leaving a socket file that refuses connects.
fn stale(socket: &Path) {
    drop(UnixListener::bind(socket).unwrap());
}

#[test]
fn a_spent_budget_is_an_unreachable_controller_naming_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("missing.sock");
    stale(&sock);
    let err = call(
        &redial_env("1"),
        &Target::Socket(sock),
        &show(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(
        matches!(err, ClientError::Unreachable { .. })
            && text.contains("not back after 1s")
            && text.contains("missing.sock"),
        "{text}"
    );
}

fn event(cursor: i64) -> String {
    format!(r#"{{"frame":"event","cursor":{cursor},"kind":"copied","payload":{{}},"ts":1}}"#)
}

/// Serves a store of events 1 to 4 after the asked cursor; `restart_after` ends the stream
/// with `restarting` once that cursor went out.
fn store_watch(restart_after: Option<i64>) -> Script {
    Box::new(move |peer: &mut Peer| {
        let watch: serde_json::Value = serde_json::from_str(&peer.take_request()).unwrap();
        let since = watch["since"].as_i64().unwrap();
        for cursor in since + 1..=4 {
            peer.say(&event(cursor));
            if Some(cursor) == restart_after {
                peer.say(r#"{"frame":"error","kind":"restarting","message":"draining"}"#);
                return;
            }
        }
    })
}

#[test]
fn events_resume_after_the_last_cursor_once_the_controller_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let lines = controller(&sock, vec![store_watch(Some(2)), store_watch(None)]);
    let mut out = Vec::new();
    watch(
        &redial_env("10"),
        &Target::Socket(sock),
        0,
        &WatchFilter::default(),
        false,
        &mut out,
    )
    .unwrap();
    let cursors: Vec<i64> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["cursor"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(cursors, [1, 2, 3, 4]);
    assert_eq!(lines.lock().unwrap().len(), 2);
}

#[test]
fn a_missing_socket_path_fails_at_once_with_the_unreachable_message() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("missing.sock");
    let began = Instant::now();
    let err = call(
        &Env::default(),
        &Target::Socket(sock.clone()),
        &show(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert!(began.elapsed() < Duration::from_secs(1));
    let ClientError::Unreachable { detail, .. } = &err else {
        panic!("{err}");
    };
    assert!(detail.contains("No such file"), "{detail}");
    assert_eq!(
        err.to_string(),
        format!("controller at {} is unreachable: {detail}", sock.display())
    );
}

#[test]
fn a_failed_request_write_is_an_error_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let deaf: Script = Box::new(|peer: &mut Peer| {
        peer.reader.read_line(&mut String::new()).unwrap();
        // A peer that shut its read side makes the request write fail with a broken pipe.
        peer.writer.shutdown(std::net::Shutdown::Read).unwrap();
        peer.say(&hello());
        std::thread::sleep(Duration::from_millis(300));
    });
    let lines = controller(&sock, vec![deaf, respond_ok()]);
    let err = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &show(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("write to controller")),
        "{err}"
    );
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        lines.lock().unwrap().is_empty(),
        "the request was sent again"
    );
}

/// Answers the request with `first`, then `restarting`.
fn then_restarting(first: &'static [u8]) -> Script {
    Box::new(move |peer: &mut Peer| {
        peer.take_request();
        peer.writer.write_all(first).unwrap();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    })
}

const BYTES: &[u8] = b"{\"frame\":\"bytes\",\"id\":\"1\",\"len\":2}\nab";
const PROGRESS: &[u8] = b"{\"frame\":\"progress\",\"id\":\"1\",\"message\":\"working\"}\n";

#[test]
fn a_session_that_passed_output_on_is_not_resent_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let lines = controller(&sock, vec![then_restarting(BYTES), respond_ok()]);
    let mut got = Vec::new();
    let err = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &show(),
        |_| {},
        |data| got.extend_from_slice(data),
    )
    .unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("draining")),
        "{err}"
    );
    assert_eq!(got, b"ab");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(lines.lock().unwrap().len(), 1);
}

#[test]
fn a_request_that_showed_progress_is_not_resent_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let lines = controller(&sock, vec![then_restarting(PROGRESS), respond_ok()]);
    let err = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &show(),
        |_| {},
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("draining")),
        "{err}"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(lines.lock().unwrap().len(), 1);
}

#[test]
fn a_wait_that_showed_progress_is_sent_again_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let lines = controller(&sock, vec![then_restarting(PROGRESS), respond_ok()]);
    let wait = Request::Wait(WaitReq {
        box_name: BoxName::parse("repo/one").unwrap(),
        wait_for: None,
        timeout_secs: None,
    });
    let resp = call(
        &redial_env("10"),
        &Target::Socket(sock),
        &wait,
        |_| {},
        |_| {},
    )
    .unwrap();
    assert_eq!(resp.class, ExitClass::Ok);
    assert_eq!(lines.lock().unwrap().len(), 2);
}

#[test]
fn the_default_redial_waits_up_to_sixty_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let restarting: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    });
    controller(&sock, vec![restarting, respond_ok()]);
    let mut notes = Vec::new();
    call(
        &Env::default(),
        &Target::Socket(sock),
        &show(),
        |m| notes.push(m.to_owned()),
        |_| {},
    )
    .unwrap();
    assert!(
        notes.len() == 1 && notes[0].ends_with("dialing again for up to 60s"),
        "{notes:?}"
    );
}

#[test]
fn a_malformed_redial_setting_is_a_usage_error_before_any_dial() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let lines = controller(&sock, vec![respond_ok()]);
    for bad in ["soon", "-1", "1.5"] {
        let err = call(
            &redial_env(bad),
            &Target::Socket(sock.clone()),
            &show(),
            |_| {},
            |_| {},
        )
        .unwrap_err();
        assert!(
            matches!(&err, ClientError::Usage(m) if m.contains("MOOR_CONTROLLER_REDIAL_SECS")),
            "{bad}: {err}"
        );
        let resp = failure_response("show", &err);
        assert_eq!(resp.class, ExitClass::Usage);
        assert_eq!(resp.error.unwrap().kind, ErrorKind::Usage);
    }
    assert!(lines.lock().unwrap().is_empty());
}

/// A fake ssh that counts its runs in `count`, exits `code` on the first run and answers the
/// request on later ones.
fn ssh_failing_once(dir: &Path, code: u8) -> (Env, std::path::PathBuf) {
    let count = dir.join("runs");
    let body = format!(
        "echo run >> '{count}'\n[ \"$(wc -l < '{count}')\" -eq 1 ] && exit {code}\n\
         read l\necho '{}'\nread l\necho '{}'",
        hello(),
        r#"{"frame":"response","id":"1","envelope":ENV}"#.replace("ENV", &ok_envelope()),
        count = count.display(),
    );
    let env = fake_ssh(dir, &body);
    let env = Env::from_pairs([("MOOR_SSH_BIN", env.get("MOOR_SSH_BIN").unwrap().to_owned())]);
    (env, count)
}

fn runs(count: &Path) -> usize {
    std::fs::read_to_string(count).unwrap().lines().count()
}

#[test]
fn over_ssh_a_refused_remote_socket_or_a_close_before_the_hello_is_dialed_again() {
    for code in [moor_client::ATTACH_REFUSED_EXIT, 0] {
        let dir = tempfile::tempdir().unwrap();
        let (env, count) = ssh_failing_once(dir.path(), code);
        let resp = call(&env, &ssh_target(), &show(), |_| {}, |_| {}).unwrap();
        assert_eq!(resp.class, ExitClass::Ok, "exit {code}");
        assert_eq!(runs(&count), 2, "exit {code}");
    }
}

#[test]
fn over_ssh_a_missing_remote_socket_or_a_failed_ssh_fails_at_once() {
    for code in [1, 255] {
        let dir = tempfile::tempdir().unwrap();
        let (env, count) = ssh_failing_once(dir.path(), code);
        let err = call(&env, &ssh_target(), &show(), |_| {}, |_| {}).unwrap_err();
        assert!(
            matches!(&err, ClientError::Unreachable { .. }),
            "{code}: {err}"
        );
        assert_eq!(runs(&count), 1, "exit {code} was dialed again");
    }
}
