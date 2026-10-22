#![allow(clippy::indexing_slicing, reason = "tests index fixed fixtures")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use moor_core::controller::WatchFilter;
use moor_core::ExitClass;

use super::{hello, ok_envelope, show};
use crate::client::{call, watch, ClientError, Env, Target};

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

#[test]
fn a_spent_budget_is_an_unreachable_controller_naming_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let err = call(
        &redial_env("1"),
        &Target::Socket(dir.path().join("missing.sock")),
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
