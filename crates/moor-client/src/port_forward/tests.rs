//! A forward against a controller stand-in that upper-cases each connection's bytes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests assert on fixed fixtures"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use moor_core::controller::{parse_frame, write_bytes, ClientFrame, ServerFrame, PROTOCOL};
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};

use super::{run, Event, Say, Spec};
use crate::Client;

/// What the stand-in saw and how it answers.
#[derive(Default)]
struct Script {
    /// Every request kind and the counts a close carried, in order.
    seen: Mutex<Vec<String>>,
    /// The words a `forward` is refused with; none carries it.
    refuse_forward: Mutex<Option<String>>,
}

/// Serves connections on `socket` until the process ends.
fn controller(socket: &Path, script: &Arc<Script>) {
    let listener = UnixListener::bind(socket).unwrap();
    let script = Arc::clone(script);
    thread::spawn(move || {
        while let Ok((conn, _)) = listener.accept() {
            let script = Arc::clone(&script);
            thread::spawn(move || serve(&conn, &script));
        }
    });
}

fn send(conn: &mut UnixStream, frame: &ServerFrame) {
    conn.write_all(frame.to_line().as_bytes()).unwrap();
}

fn serve(conn: &UnixStream, script: &Script) {
    let mut out = conn.try_clone().unwrap();
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    send(&mut out, &ServerFrame::Hello { protocol: PROTOCOL });
    line.clear();
    reader.read_line(&mut line).unwrap();
    let Ok(ClientFrame::Request { id, request }) = parse_frame(line.trim_end()) else {
        return;
    };
    let cmd = request.cmd();
    let envelope = match *request {
        Request::Forward(_) => {
            script.seen.lock().unwrap().push("forward".to_owned());
            let refused = script.refuse_forward.lock().unwrap().clone();
            refused.map_or_else(
                || {
                    echo(&mut reader, &mut out, &id);
                    Response::ok(cmd, None)
                },
                |why| refusal(&why),
            )
        }
        Request::ForwardClose(r) => {
            let counts = (r.connections, r.bytes_in, r.bytes_out);
            script
                .seen
                .lock()
                .unwrap()
                .push(format!("close {counts:?}"));
            Response::ok(cmd, None)
        }
        _ => {
            script.seen.lock().unwrap().push(cmd.to_owned());
            Response::ok(cmd, None)
        }
    };
    send(&mut out, &ServerFrame::Response { id, envelope });
}

fn refusal(why: &str) -> Response {
    let mut resp = Response::ok("forward", None);
    resp.error = Some(ErrorView {
        kind: ErrorKind::Refused,
        message: why.to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
        detail: None,
    });
    resp
}

/// Sends each bytes frame back upper-cased until the client's eof.
fn echo(reader: &mut BufReader<UnixStream>, out: &mut UnixStream, id: &str) {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        match parse_frame(line.trim_end()) {
            Ok(ClientFrame::Bytes { len, .. }) => {
                let mut data = vec![0u8; len];
                reader.read_exact(&mut data).unwrap();
                write_bytes(out, id, &data.to_ascii_uppercase()).unwrap();
            }
            _ => return,
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spec(port: u16) -> Spec {
    Spec {
        box_name: "t".to_owned(),
        port,
        local_port: Some(port),
    }
}

fn connect(port: u16) -> TcpStream {
    let mut tries = 0;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(conn) => return conn,
            Err(e) => assert!(tries < 100, "nothing listens on {port}: {e}"),
        }
        tries += 1;
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_browser_connection_reaches_the_box_and_the_close_carries_the_counts() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    controller(&sock, &script);
    let (client, port) = (Client::new(&sock), free_port());
    let (stop, events) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::new())),
    );
    let say: Say = {
        let events = Arc::clone(&events);
        Arc::new(move |e: Event| events.lock().unwrap().push(e))
    };
    let forward = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || run(&client, &spec(port), &stop, say))
    };
    let mut browser = connect(port);
    browser.write_all(b"hello").unwrap();
    browser.shutdown(Shutdown::Write).unwrap();
    let mut back = Vec::new();
    browser.read_to_end(&mut back).unwrap();
    assert_eq!(back, b"HELLO");
    stop.store(true, Ordering::SeqCst);
    let counts = forward.join().unwrap().unwrap();
    assert_eq!(
        (counts.connections, counts.bytes_in, counts.bytes_out),
        (1, 5, 5)
    );
    let seen = script.seen.lock().unwrap().clone();
    assert_eq!(seen, ["forward-open", "forward", "close (1, 5, 5)"]);
    let events = events.lock().unwrap().clone();
    assert_eq!(events.first(), Some(&Event::Opened { local_port: port }));
    assert!(events.contains(&Event::ConnectionOpen(1)), "{events:?}");
}

#[test]
fn a_local_port_that_is_taken_fails_before_the_box_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    controller(&sock, &script);
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let say: Say = Arc::new(|_| {});
    let err = run(&Client::new(&sock), &spec(port), &stop, say).unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "localhost:{port} is in use on this computer; stop what holds it or pass --local-port"
        )
    );
    assert!(script.seen.lock().unwrap().is_empty());
}

#[test]
fn a_box_that_closed_ends_the_forward_on_the_next_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    *script.refuse_forward.lock().unwrap() = Some("t is closed; the forward ended".to_owned());
    controller(&sock, &script);
    let (client, port) = (Client::new(&sock), free_port());
    let stop = Arc::new(AtomicBool::new(false));
    let say: Say = Arc::new(|_| {});
    let forward = thread::spawn(move || run(&client, &spec(port), &stop, say));
    drop(connect(port));
    let err = forward.join().unwrap().unwrap_err();
    assert_eq!(err.0, "t is closed; the forward ended");
}

#[test]
fn a_local_port_held_on_ipv6_loopback_alone_is_taken_too() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    controller(&sock, &script);
    let Ok(held) = TcpListener::bind("[::1]:0") else {
        return;
    };
    let port = held.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let say: Say = Arc::new(|_| {});
    let err = run(&Client::new(&sock), &spec(port), &stop, say).unwrap_err();
    assert!(
        err.0.starts_with(&format!("localhost:{port} is in use")),
        "{err}"
    );
    assert!(script.seen.lock().unwrap().is_empty());
}

#[test]
fn local_port_zero_reports_and_records_the_port_it_bound() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    controller(&sock, &script);
    let (stop, events) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::new())),
    );
    let say: Say = {
        let events = Arc::clone(&events);
        Arc::new(move |e: Event| events.lock().unwrap().push(e))
    };
    let client = Client::new(&sock);
    let forward = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let spec = Spec {
                box_name: "t".to_owned(),
                port: 5177,
                local_port: Some(0),
            };
            run(&client, &spec, &stop, say)
        })
    };
    let mut bound = None;
    for _ in 0..100 {
        if let Some(Event::Opened { local_port }) = events.lock().unwrap().first() {
            bound = Some(*local_port);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let bound = bound.unwrap();
    assert_ne!(bound, 0);
    drop(connect(bound));
    stop.store(true, Ordering::SeqCst);
    forward.join().unwrap().unwrap();
}

#[test]
fn a_connection_over_the_cap_is_dropped_and_the_person_is_told() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("c.sock");
    let script = Arc::new(Script::default());
    controller(&sock, &script);
    let (client, port) = (Client::new(&sock), free_port());
    let (stop, events) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::new())),
    );
    let say: Say = {
        let events = Arc::clone(&events);
        Arc::new(move |e: Event| events.lock().unwrap().push(e))
    };
    let forward = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || run(&client, &spec(port), &stop, say))
    };
    let held: Vec<TcpStream> = (0..=super::MAX_CONNECTIONS)
        .map(|_| {
            let conn = connect(port);
            thread::sleep(Duration::from_millis(5));
            conn
        })
        .collect();
    for _ in 0..100 {
        if events.lock().unwrap().contains(&Event::Full) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(events.lock().unwrap().contains(&Event::Full));
    drop(held);
    stop.store(true, Ordering::SeqCst);
    forward.join().unwrap().unwrap();
}
