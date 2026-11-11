//! A person's forward end to end: the real client's listener, the controller socket, the
//! `forward` ops and a fake driver whose Box echoes what a connection sends.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown as TcpShutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_client::port_forward::{run, Counts, Error, Event, Say, Spec};
use moor_client::Client;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::user::Caller;
use moor_core::Request;

use super::super::drain::{Admitted, Drain};
use super::super::hangup::Hangup;
use super::exec::piped_with;
use super::{serve, Peer};
use crate::cx::Cx;
use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// A controller socket and the drain its connections belong to.
struct Served {
    /// The socket's path.
    sock: PathBuf,
    /// Admits each connection; beginning it is a controller roll.
    drain: Arc<Drain>,
}

/// Serves every connection on a socket as `caller`, with the fixture's store and driver, through
/// the controller's own authorization and request handling.
fn controller(f: &Fx, caller: &Caller) -> Served {
    let sock = f.dir.path().join("c.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let mut spare: Vec<Cx> = (0..8).map(|_| f.cx.detached().unwrap()).collect();
    let drain = Arc::new(Drain::default());
    let (held, dir, caller) = (
        Arc::clone(&drain),
        f.dir.path().join("vault"),
        caller.clone(),
    );
    thread::spawn(move || {
        while let Ok((conn, _)) = listener.accept() {
            let admitted = held.hold().unwrap_or_else(Admitted::unwatched);
            let (cx, dir, caller) = (spare.pop().unwrap(), dir.clone(), caller.clone());
            thread::spawn(move || serve_one(conn, (cx, &dir, caller), admitted));
        }
    });
    Served { sock, drain }
}

/// Answers the one request of `conn` the way the controller does.
fn serve_one(conn: UnixStream, (cx, dir, caller): (Cx, &Path, Caller), admitted: Admitted) {
    let reader = BufReader::new(conn.try_clone().unwrap());
    let mut peer = Peer::local_in(dir);
    peer.caller = Ok(caller);
    peer.hangup = Hangup::of(&conn).ok();
    peer.conn = admitted;
    let cx = Mutex::new(Some(cx));
    serve(
        reader,
        conn,
        &peer.conn,
        |id, request, duplex| {
            let moor = Moor::from_cx(cx.lock().unwrap().take().unwrap());
            let grant = match moor.authorize(&peer.caller, None, &request) {
                Ok(grant) => grant,
                Err(e) => return Some(e.into_response(request.cmd(), vec![])),
            };
            let moor = moor.granted(grant);
            Some(match request {
                Request::Forward(_) => piped_with(&moor, &peer, id, &request, duplex),
                other => moor.call(other),
            })
        },
        |_, _, _, _, _| {},
    );
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A forward of port 5177 of Box `t` on `local`, running on a thread.
struct Forward {
    stop: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<Event>>>,
    thread: JoinHandle<Result<Counts, Error>>,
}

fn forward(sock: PathBuf, local: u16) -> Forward {
    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let say: Say = {
        let events = Arc::clone(&events);
        Arc::new(move |e| events.lock().unwrap().push(e))
    };
    let spec = Spec {
        box_name: "t".to_owned(),
        port: 5177,
        local_port: Some(local),
    };
    let thread = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || run(&Client::new(sock), &spec, &stop, say))
    };
    Forward {
        stop,
        events,
        thread,
    }
}

/// Sends `bytes` as a browser would and returns what comes back until the connection ends.
fn browse(local: u16, bytes: &[u8]) -> Vec<u8> {
    let mut tries = 0;
    let mut conn = loop {
        match TcpStream::connect(("127.0.0.1", local)) {
            Ok(conn) => break conn,
            Err(e) => assert!(tries < 100, "nothing listens on {local}: {e}"),
        }
        tries += 1;
        thread::sleep(Duration::from_millis(20));
    };
    conn.write_all(bytes).unwrap();
    conn.shutdown(TcpShutdown::Write).unwrap();
    let mut back = Vec::new();
    conn.read_to_end(&mut back).unwrap();
    back
}

fn events(f: &Fx, kind: &str) -> Vec<serde_json::Value> {
    let mut rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.sort_by_key(|e| e.id);
    rows.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_browser_request_through_localhost_reaches_the_box_and_the_forward_is_recorded() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let (sock, local) = (controller(&f, &Caller::Operator(1000)).sock, free_port());
    let fwd = forward(sock, local);
    let request = b"GET /login HTTP/1.1\r\nHost: localhost:5177\r\n\r\n";
    assert_eq!(browse(local, request), request);
    assert_eq!(f.driver.port_bytes(), request);
    fwd.stop.store(true, Ordering::SeqCst);
    let counts = fwd.thread.join().unwrap().unwrap();
    assert_eq!(
        (counts.connections, counts.bytes_in, counts.bytes_out),
        (1, request.len() as u64, request.len() as u64)
    );
    assert_eq!(
        events(&f, "port_forward_opened"),
        [serde_json::json!({"port": 5177, "local_port": local})]
    );
    let closed = events(&f, "port_forward_closed");
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(
        closed.first().and_then(|c| c.get("connections")),
        Some(&1.into())
    );
    assert!(f.cx.traffic().recent(f.id, f.cx.clock().now()));
}

#[test]
fn a_box_that_sleeps_refuses_the_next_connection_and_the_listener_stays() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let (sock, local) = (controller(&f, &Caller::Operator(1000)).sock, free_port());
    let fwd = forward(sock, local);
    assert_eq!(browse(local, b"one"), b"one");
    f.cx.store()
        .unwrap()
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::sleep(false))
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
    assert_eq!(browse(local, b"two"), b"");
    assert_eq!(f.driver.port_bytes(), b"one");
    assert_eq!(
        browse(local, b"three"),
        b"",
        "the listener stays for the next connection"
    );
    let asleep = |e: &&Event| {
        matches!(e, Event::ConnectionRefused { reason, .. }
            if reason == "t is asleep; open the forward again to wake it")
    };
    for _ in 0..100 {
        if fwd.events.lock().unwrap().iter().filter(asleep).count() == 2 {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let events = fwd.events.lock().unwrap().clone();
    assert_eq!(events.iter().filter(asleep).count(), 2, "{events:?}");
    fwd.stop.store(true, Ordering::SeqCst);
    fwd.thread.join().unwrap().unwrap();
}

#[test]
fn a_forward_through_a_controller_roll_finishes_and_the_next_connection_reaches_the_new_one() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let served = controller(&f, &Caller::Operator(1000));
    let local = free_port();
    let fwd = forward(served.sock.clone(), local);
    let mut tries = 0;
    let mut conn = loop {
        match TcpStream::connect(("127.0.0.1", local)) {
            Ok(conn) => break conn,
            Err(e) => assert!(tries < 100, "nothing listens on {local}: {e}"),
        }
        tries += 1;
        thread::sleep(Duration::from_millis(20));
    };
    conn.write_all(b"one").unwrap();
    let mut got = [0u8; 3];
    conn.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"one");
    served.drain.begin();
    conn.write_all(b"two").unwrap();
    conn.read_exact(&mut got).unwrap();
    assert_eq!(
        &got, b"two",
        "the connection in flight carries on through the roll"
    );
    conn.shutdown(TcpShutdown::Write).unwrap();
    let mut rest = Vec::new();
    conn.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, b"");
    assert!(
        served.drain.hold().is_none(),
        "the rolling controller admits no more"
    );
    assert_eq!(
        browse(local, b"three"),
        b"three",
        "the next connection is served by whatever controller takes over"
    );
    assert_eq!(f.driver.port_bytes(), b"onetwothree");
    fwd.stop.store(true, Ordering::SeqCst);
    fwd.thread.join().unwrap().unwrap();
}

#[test]
fn another_users_box_is_refused_at_open_and_nothing_listens_for_it() {
    let f = ready_with(FakeDriver::new().on_a_node(&[5177]));
    let served = controller(&f, &Caller::Local(2000));
    let spec = Spec {
        box_name: "t".to_owned(),
        port: 5177,
        local_port: Some(free_port()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let say: Say = Arc::new(|_| {});
    let err = run(&Client::new(served.sock), &spec, &stop, say).unwrap_err();
    assert_eq!(err.0, "not your Box");
    assert!(events(&f, "port_forward_opened").is_empty());
    assert!(
        events(&f, "port_forward_refused").is_empty(),
        "an authz refusal writes nothing"
    );
}
