#![allow(
    clippy::disallowed_methods,
    clippy::panic,
    reason = "tests time the redial with the wall clock and fail loudly"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{bytes_frame, hello, ok_envelope, show};
use crate::{Client, ClientError, Redial, Timeouts, WatchFilter, WatchItem};
use moor_core::controller::FrameErrorKind;
use moor_core::request::{Request, WaitReq};
use moor_core::response::Response;
use moor_core::BoxName;

/// The far end of one accepted connection; records every line it reads.
struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Peer {
    fn line(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        let line = line.trim_end().to_owned();
        self.lines.lock().unwrap().push(line.clone());
        line
    }

    fn say(&mut self, line: &str) {
        writeln!(self.writer, "{line}").unwrap();
    }

    /// Reads the hello and the request, answering the hello; returns the request line.
    fn take_request(&mut self) -> String {
        self.line();
        self.say(&hello());
        self.line()
    }
}

type Script = Box<dyn FnOnce(&mut Peer) + Send>;

/// What a stand-in controller saw.
#[derive(Clone, Default)]
struct Seen {
    accepted: Arc<AtomicUsize>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Seen {
    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn requests(&self) -> usize {
        let lines = self.lines.lock().unwrap();
        lines
            .iter()
            .filter(|l| l.starts_with(r#"{"frame":"request""#))
            .count()
    }

    fn watches(&self) -> Vec<String> {
        let lines = self.lines.lock().unwrap();
        lines
            .iter()
            .filter(|l| l.starts_with(r#"{"frame":"watch""#))
            .cloned()
            .collect()
    }
}

/// Binds `socket` now, or after `delay` when there is one (replacing a stale socket), then
/// runs one script per accepted connection in order. A connection beyond the scripts blocks
/// in accept, so it is seen only as a count.
fn controller(socket: &Path, delay: Duration, scripts: Vec<Script>) -> Seen {
    let seen = Seen::default();
    let (socket, out) = (socket.to_owned(), seen.clone());
    let bound = delay
        .is_zero()
        .then(|| UnixListener::bind(&socket).unwrap());
    std::thread::spawn(move || {
        let listener = bound.unwrap_or_else(|| {
            std::thread::sleep(delay);
            let _ = std::fs::remove_file(&socket);
            UnixListener::bind(socket).unwrap()
        });
        for script in scripts {
            let (stream, _) = listener.accept().unwrap();
            out.accepted.fetch_add(1, Ordering::SeqCst);
            let mut peer = Peer {
                reader: BufReader::new(stream.try_clone().unwrap()),
                writer: stream,
                lines: Arc::clone(&out.lines),
            };
            script(&mut peer);
        }
        let _ = listener.accept();
        out.accepted.fetch_add(1, Ordering::SeqCst);
    });
    seen
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

fn quick() -> Redial {
    Redial {
        budget: Duration::from_secs(10),
        refused: Duration::from_secs(10),
        first_pause: Duration::from_millis(20),
        max_pause: Duration::from_millis(200),
    }
}

fn socket(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("c.sock")
}

#[test]
fn a_controller_that_rebinds_a_stale_socket_after_two_seconds_answers_the_request_once() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    stale(&sock);
    let seen = controller(&sock, Duration::from_secs(2), vec![respond_ok()]);
    let began = Instant::now();
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert!(began.elapsed() >= Duration::from_secs(2));
    assert_eq!(seen.requests(), 1);
}

#[test]
fn a_request_cut_off_by_eof_after_the_send_is_an_error_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let eof: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
    });
    let seen = controller(&sock, Duration::ZERO, vec![eof, respond_ok()]);
    let err = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap_err();
    assert!(
        matches!(&err, ClientError::Protocol(m) if m.contains("closed the connection")),
        "{err}"
    );
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 1);
}

#[test]
fn a_request_cut_off_by_a_reset_after_the_send_is_an_error_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let reset: Script = Box::new(|peer: &mut Peer| {
        peer.line();
        peer.say(&hello());
        // Closing with the request unread resets the client's connection.
        std::thread::sleep(Duration::from_millis(200));
    });
    let seen = controller(&sock, Duration::ZERO, vec![reset, respond_ok()]);
    let err = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap_err();
    assert!(matches!(&err, ClientError::Io(_)), "{err}");
    assert_eq!(seen.accepted(), 1);
}

#[test]
fn a_restarting_frame_for_the_request_sends_it_once_more() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let restarting: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    });
    let seen = controller(&sock, Duration::ZERO, vec![restarting, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 2);
    assert_eq!(seen.accepted(), 2);
}

/// Binds `socket` and closes the listener, leaving a socket file that refuses connects.
fn stale(socket: &Path) {
    drop(UnixListener::bind(socket).unwrap());
}

/// Answers a watch from a store of events 1 to 5: the first connection delivers up to 2 and
/// then restarts, later ones deliver everything after the asked cursor.
fn store_watch(restart_after: Option<i64>) -> Script {
    Box::new(move |peer: &mut Peer| {
        let watch: serde_json::Value = serde_json::from_str(&peer.take_request()).unwrap();
        let since = watch["since"].as_i64().unwrap_or(0);
        for cursor in since + 1..=5 {
            peer.say(&super::event(cursor, ""));
            if Some(cursor) == restart_after {
                peer.say(r#"{"frame":"error","kind":"restarting","message":"draining; last delivered cursor 2"}"#);
                return;
            }
        }
        peer.say(r#"{"frame":"caught-up","cursor":5}"#);
    })
}

#[test]
fn a_watch_ended_by_restarting_resumes_after_its_last_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let seen = controller(
        &sock,
        Duration::ZERO,
        vec![store_watch(Some(2)), store_watch(None)],
    );
    let watch = Client::new(&sock)
        .with_redial(quick())
        .watch(0, WatchFilter::default(), false)
        .unwrap();
    let cursors: Vec<i64> = watch
        .map(|item| match item.unwrap() {
            WatchItem::Event(e) => e.cursor,
            WatchItem::CaughtUp { cursor } => -cursor,
        })
        .collect();
    assert_eq!(cursors, [1, 2, 3, 4, 5, -5]);
    assert_eq!(seen.watches().len(), 2);
}

#[test]
fn a_stopped_watch_ends_while_it_redials() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let restart_then_gone: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","kind":"restarting","message":"draining"}"#);
    });
    controller(&sock, Duration::ZERO, vec![restart_then_gone]);
    let redial = Redial {
        budget: Duration::from_secs(30),
        refused: Duration::from_secs(30),
        first_pause: Duration::from_secs(10),
        max_pause: Duration::from_secs(10),
    };
    let mut watch = Client::new(&sock)
        .with_redial(redial)
        .watch(0, WatchFilter::default(), true)
        .unwrap();
    let stop = watch.stopper();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.stop();
    });
    let began = Instant::now();
    assert!(watch.next().is_none());
    assert!(began.elapsed() < Duration::from_secs(5));
    stopper.join().unwrap();
}

#[test]
fn no_redial_returns_a_restarting_frame_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let restarting: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    });
    controller(&sock, Duration::ZERO, vec![restarting]);
    let err = Client::new(&sock)
        .with_redial(Redial::NEVER)
        .request(show(), |_| {})
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Frame {
                kind: FrameErrorKind::Restarting,
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn the_id_less_restarting_frame_resends_the_request_that_got_no_response() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let closed: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","kind":"restarting","message":"connection ending"}"#);
    });
    let seen = controller(&sock, Duration::ZERO, vec![closed, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 2);
}

#[test]
fn a_wait_cut_by_two_restarting_frames_is_resent_once() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let cut: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"wait cut"}"#);
        // The client may hang up after the first frame, so this write can meet a closed socket.
        let ending = r#"{"frame":"error","kind":"restarting","message":"connection ending"}"#;
        let _ = writeln!(peer.writer, "{ending}");
    });
    let seen = controller(&sock, Duration::ZERO, vec![cut, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 2);
    assert_eq!(seen.accepted(), 2);
}

#[test]
fn a_response_followed_by_restarting_is_kept_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let answered: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(&format!(
            r#"{{"frame":"response","id":"1","envelope":{}}}"#,
            ok_envelope()
        ));
        peer.say(r#"{"frame":"error","kind":"restarting","message":"connection ending"}"#);
    });
    let seen = controller(&sock, Duration::ZERO, vec![answered, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 1);
}

#[test]
fn a_failed_request_write_is_an_error_and_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let deaf: Script = Box::new(|peer: &mut Peer| {
        peer.line();
        // A peer that shut its read side makes the request write fail with a broken pipe.
        peer.writer.shutdown(std::net::Shutdown::Read).unwrap();
        peer.say(&hello());
        std::thread::sleep(Duration::from_millis(300));
    });
    let seen = controller(&sock, Duration::ZERO, vec![deaf, respond_ok()]);
    let err = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap_err();
    assert!(matches!(&err, ClientError::Io(_)), "{err}");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(seen.accepted(), 1);
}

fn wait() -> Request {
    Request::Wait(WaitReq {
        box_name: BoxName::parse("repo/one").unwrap(),
        wait_for: None,
        timeout_secs: None,
    })
}

/// Answers the request with one progress note, then `restarting`.
fn progress_then_restarting() -> Script {
    Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.say(r#"{"frame":"progress","id":"1","message":"working"}"#);
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    })
}

#[test]
fn a_request_that_showed_progress_is_not_resent_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let seen = controller(
        &sock,
        Duration::ZERO,
        vec![progress_then_restarting(), respond_ok()],
    );
    let mut notes = Vec::new();
    let err = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |m| notes.push(m.to_owned()))
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Frame {
                kind: FrameErrorKind::Restarting,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(notes, ["working"]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 1);
}

#[test]
fn a_wait_that_showed_progress_is_sent_again_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let seen = controller(
        &sock,
        Duration::ZERO,
        vec![progress_then_restarting(), respond_ok()],
    );
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(wait(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 2);
}

#[test]
fn a_read_that_passed_bytes_on_is_not_resent_after_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let cut: Script = Box::new(|peer: &mut Peer| {
        peer.take_request();
        peer.writer.write_all(&bytes_frame(b"ab")).unwrap();
        peer.say(r#"{"frame":"error","id":"1","kind":"restarting","message":"draining"}"#);
    });
    let seen = controller(&sock, Duration::ZERO, vec![cut, respond_ok()]);
    let mut got = Vec::new();
    let err = Client::new(&sock)
        .with_redial(quick())
        .read(show(), |data| {
            got.extend_from_slice(data);
            Ok(())
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            ClientError::Frame {
                kind: FrameErrorKind::Restarting,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(got, b"ab");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 1);
}

#[test]
fn a_connection_closed_before_the_hello_reply_is_dialed_again() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let closed: Script = Box::new(|peer: &mut Peer| {
        peer.line();
    });
    let seen = controller(&sock, Duration::ZERO, vec![closed, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 2);
}

#[test]
fn a_connection_reset_before_the_hello_reply_is_dialed_again() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let reset: Script = Box::new(|_: &mut Peer| {
        // Closing with the hello unread resets the client's connection.
        std::thread::sleep(Duration::from_millis(200));
    });
    let seen = controller(&sock, Duration::ZERO, vec![reset, respond_ok()]);
    let resp = Client::new(&sock)
        .with_redial(quick())
        .request(show(), |_| {})
        .unwrap();
    assert_eq!(resp, Response::ok("show", None));
    assert_eq!(seen.requests(), 1);
    assert_eq!(seen.accepted(), 2);
}

#[test]
fn a_stop_ends_a_watch_whose_redial_waits_on_a_held_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    let listener = UnixListener::bind(&sock).unwrap();
    // After the restarting end the listener stays open and accepts nothing more, so the
    // redial's connect queues and its hello is never answered, as on a held socket.
    let held = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut peer = Peer {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            lines: Arc::default(),
        };
        peer.take_request();
        peer.say(r#"{"frame":"error","kind":"restarting","message":"draining"}"#);
        std::thread::sleep(Duration::from_secs(8));
        drop(listener);
    });
    let mut watch = Client::new(&sock)
        .with_timeouts(Timeouts {
            read: Duration::from_secs(5),
            write: Duration::from_secs(5),
        })
        .with_redial(quick())
        .watch(0, WatchFilter::default(), true)
        .unwrap();
    let stop = watch.stopper();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        stop.stop();
    });
    let began = Instant::now();
    assert!(watch.next().is_none());
    let took = began.elapsed();
    assert!(
        took < Duration::from_secs(3),
        "the stop waited out the hello: {took:?}"
    );
    stopper.join().unwrap();
    drop(held);
}

mod budget;
