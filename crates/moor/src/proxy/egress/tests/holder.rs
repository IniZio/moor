//! Box calls to the machine service go to the runner that holds the machine's link, as the
//! ledger names it, and on to the new holder when that runner no longer holds it.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::request::{MachineLink, RunnerAddr};
use moor_core::seam::clock::Ts;
use moor_core::user::Identity;
use moor_core::{HoldOp, HoldValue};
use tokio_tungstenite::tungstenite::Message;

use super::{open_as, Server, World};
use crate::proxy::egress::MachineRoute;

/// The machine service's host name, as Boxes request it.
const MACHINE: &str = "machine.moor.internal";
/// The body of the Box's call.
const CALL: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call"}"#;

/// What a fake runner does with a request, given its head.
enum Reply {
    /// Reads the body, then answers.
    Answer(&'static str),
    /// Answers from the head alone and closes without reading the body.
    Unread(&'static str),
    /// Reads the request and closes without answering.
    HangUp,
}

/// How a fake runner picks its reply.
type Answer = Box<dyn Fn(&str) -> Reply + Send>;

/// A fake runner or service on `addr` that keeps each request it reads, head and body.
struct Fake {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn start(listener: TcpListener, answer: Answer) -> Self {
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = Arc::clone(&seen);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                let Some(head) = read_head(&mut conn) else {
                    continue;
                };
                let reply = answer(&head);
                let request = match reply {
                    Reply::Unread(_) => head,
                    Reply::Answer(_) | Reply::HangUp => match read_body(&mut conn, head) {
                        Some(request) => request,
                        None => continue,
                    },
                };
                kept.lock().unwrap().push(request);
                if let Reply::Answer(text) | Reply::Unread(text) = reply {
                    drop(conn.write_all(text.as_bytes()));
                }
            }
        });
        Self { addr, seen }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// One request's head; none for a connection that sent nothing.
fn read_head(conn: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
        head.extend_from_slice(&byte);
    }
    String::from_utf8(head).ok().filter(|h| !h.is_empty())
}

/// `head` and the `Content-Length` body after it.
fn read_body(conn: &mut TcpStream, head: String) -> Option<String> {
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .map_or(0, |n| n.trim().parse::<usize>().unwrap());
    let mut body = vec![0_u8; len];
    conn.read_exact(&mut body).ok()?;
    Some(head + &String::from_utf8(body).unwrap())
}

const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi";
const ELSEWHERE: &str = "HTTP/1.1 421 Misdirected Request\r\nContent-Length: 0\r\n\r\n";

/// Listeners on one port at 127.0.0.1, .2, .3 and .4, as pods each serve MCP on the port the
/// service forwards to; .4 is closed again, a runner that is gone.
fn pods() -> ([TcpListener; 3], SocketAddr) {
    loop {
        let service = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = service.local_addr().unwrap().port();
        let bind = |last: u8| TcpListener::bind((Ipv4Addr::new(127, 0, 0, last), port));
        if let (Ok(a), Ok(b), Ok(gone)) = (bind(2), bind(3), bind(4)) {
            let gone = gone.local_addr().unwrap();
            return ([service, a, b], gone);
        }
    }
}

/// A World whose Box holds a slot on machine `mini`, with a shared Proxy whose machine service
/// is `service`; the machine's id.
fn holding(w: &World, service: SocketAddr) -> (Server, i64) {
    let who = Identity::local(1);
    let user = w.store.enroll(&who, None).unwrap().0;
    let mini = UserMachine::parse("mini", &["macos".to_owned()]).unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    };
    let held = w
        .store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    assert!(matches!(held, HoldValue::Held { .. }), "{held:?}");
    let machine_id = w.store.claimed_machine(w.id).unwrap().unwrap().machine_id;
    let mut cfg = w.config(10.0, 10.0, 64);
    cfg.machine = Some(MachineRoute {
        key: Some("the-key".into()),
        ..MachineRoute::parse(&format!("{MACHINE}={service}")).unwrap()
    });
    (Server::start(cfg).unwrap(), machine_id)
}

/// Machine `machine_id`'s link, up since 1 s.
fn link(machine_id: i64) -> MachineLink {
    MachineLink {
        machine_id,
        online: true,
        since: Ts::from_millis(1_000),
        version: None,
        protocol: None,
        max_boxes: None,
        cache_gb: None,
        features: Vec::new(),
        mem_mib: None,
        cpus: None,
        reserve_mib: None,
        memory: None,
        warm_starts: None,
    }
}

/// `runner` reports machine `machine_id`'s link.
fn link_on(w: &World, machine_id: i64, runner: &str) {
    let runner = RunnerAddr::new(runner);
    w.store
        .set_machine_links(&runner, false, &[link(machine_id)], w.store.now())
        .unwrap();
}

/// The links address a runner on `pod` advertises: its own port, not the MCP one.
fn advertised(pod: SocketAddr) -> String {
    format!("{}:7702", pod.ip())
}

/// Posts the Box's call to the machine service through `server`; the answer.
fn call(w: &World, server: &Server) -> String {
    call_with(w, server, "", CALL.as_bytes())
}

/// Posts `body` with the extra header lines `headers` through `server`, in messages the
/// forwarder may send; the answer. A body framed by `Transfer-Encoding` in `headers` gets no
/// `Content-Length`.
fn call_with(w: &World, server: &Server, headers: &str, body: &[u8]) -> String {
    let mut tunnel = open_as(server, &w.token, &format!("{MACHINE}:80"), true).unwrap();
    let length = if headers.contains("Transfer-Encoding") {
        String::new()
    } else {
        format!("Content-Length: {}\r\n", body.len())
    };
    let head =
        format!("POST http://{MACHINE}/mcp HTTP/1.1\r\nHost: {MACHINE}\r\n{headers}{length}\r\n");
    tunnel.send(Message::binary(head.into_bytes())).unwrap();
    for chunk in body.chunks(256 * 1024) {
        tunnel.send(Message::binary(chunk.to_vec())).unwrap();
    }
    let mut got = Vec::new();
    while let Ok(msg) = tunnel.read() {
        match msg {
            Message::Binary(b) => got.extend_from_slice(&b),
            Message::Close(_) => break,
            _ => {}
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

fn ok() -> Answer {
    Box::new(|_| Reply::Answer(OK))
}

/// A fake holder that, on the first request it gets, moves machine `machine_id`'s link to
/// `new_holder` in `w`'s store and answers 421, reading the body or not.
fn relinking(w: &World, machine_id: i64, new_holder: SocketAddr, unread: bool) -> Answer {
    let (store, new_holder) = (w.store.clone(), advertised(new_holder));
    Box::new(move |head: &str| {
        assert!(head.contains("X-Moor-Holder-Only: 1\r\n"), "{head}");
        let runner = RunnerAddr::new(new_holder.as_str());
        let link = MachineLink {
            since: Ts::from_millis(2_000),
            ..link(machine_id)
        };
        store
            .set_machine_links(&runner, false, &[link], store.now())
            .unwrap();
        if unread {
            Reply::Unread(ELSEWHERE)
        } else {
            Reply::Answer(ELSEWHERE)
        }
    })
}

#[test]
fn a_box_call_goes_to_the_runner_holding_its_machines_link() {
    let w = World::new();
    let ([service, a, b], _) = pods();
    let (service, a, b) = (
        Fake::start(service, ok()),
        Fake::start(a, ok()),
        Fake::start(b, ok()),
    );
    let (server, machine_id) = holding(&w, service.addr);
    link_on(&w, machine_id, &advertised(b.addr));
    for _ in 0..4 {
        let reply = call(&w, &server);
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    }
    let seen = b.seen();
    assert_eq!(seen.len(), 4, "{seen:?}");
    let [head, ..] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(
        head.contains(&format!("X-Moor-Machine-Id: {machine_id}\r\n")),
        "{head}"
    );
    assert!(head.contains("X-Moor-Service-Key: the-key\r\n"), "{head}");
    assert!(head.ends_with(CALL), "{head}");
    assert!(service.seen().is_empty() && a.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_runner_that_lost_the_link_hands_the_call_to_the_new_holder_within_the_request() {
    let w = World::new();
    let ([service, a, b], _) = pods();
    let (service, b) = (Fake::start(service, ok()), Fake::start(b, ok()));
    let (server, machine_id) = holding(&w, service.addr);
    let a = Fake::start(a, relinking(&w, machine_id, b.addr, false));
    link_on(&w, machine_id, &advertised(a.addr));
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert_eq!(a.seen().len(), 1, "the old holder answered once");
    let seen = b.seen();
    let [head] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(!head.contains("X-Moor-Holder-Only"), "{head}");
    assert!(head.ends_with(CALL), "the body went again whole: {head}");
    assert!(service.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_holder_that_is_gone_while_the_controller_is_down_leaves_calls_to_the_service() {
    let mut w = World::new();
    let ([service, _a, _b], gone) = pods();
    let service = Fake::start(service, ok());
    let (server, machine_id) = holding(&w, service.addr);
    link_on(&w, machine_id, &advertised(gone));
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    drop(w.ledger.take());
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let seen = service.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen.iter().all(|h| h.contains("X-Moor-Machine: mini\r\n")),
        "{seen:?}"
    );
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_machine_no_runner_holds_or_one_on_a_socket_goes_to_the_service() {
    let w = World::new();
    let ([service, a, _b], _) = pods();
    let (service, a) = (Fake::start(service, ok()), Fake::start(a, ok()));
    let (server, machine_id) = holding(&w, service.addr);
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    link_on(&w, machine_id, "/run/moor/runner/runner.sock");
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let seen = service.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for head in &seen {
        assert!(head.contains("X-Moor-Machine: mini\r\n"), "{head}");
        assert!(!head.contains("X-Moor-Holder-Only"), "{head}");
    }
    assert!(a.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_holder_that_turns_away_a_large_call_unread_still_hands_it_to_the_new_holder() {
    let w = World::new();
    let ([service, a, b], _) = pods();
    let (service, b) = (Fake::start(service, ok()), Fake::start(b, ok()));
    let (server, machine_id) = holding(&w, service.addr);
    rustix::net::sockopt::set_socket_recv_buffer_size(&a, 4096).unwrap();
    let a = Fake::start(a, relinking(&w, machine_id, b.addr, true));
    link_on(&w, machine_id, &advertised(a.addr));
    let body = vec![b'x'; 1 << 20];
    let reply = call_with(&w, &server, "", &body);
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert_eq!(a.seen().len(), 1);
    let seen = b.seen();
    let [request] = seen.as_slice() else {
        unreachable!("{} requests", seen.len())
    };
    assert!(
        request.ends_with(&"x".repeat(1 << 20)),
        "the body went whole"
    );
    assert!(service.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_holder_that_hangs_up_before_answering_is_a_502_and_the_call_goes_nowhere_else() {
    let w = World::new();
    let ([service, a, b], _) = pods();
    let (service, b) = (Fake::start(service, ok()), Fake::start(b, ok()));
    let (server, machine_id) = holding(&w, service.addr);
    let a = Fake::start(a, Box::new(|_| Reply::HangUp));
    link_on(&w, machine_id, &advertised(a.addr));
    let reply = call(&w, &server);
    assert!(reply.starts_with("HTTP/1.1 502 "), "{reply}");
    assert!(reply.contains("the call may have run"), "{reply}");
    assert_eq!(a.seen().len(), 1);
    assert!(b.seen().is_empty() && service.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_holder_that_turned_a_call_away_while_the_controller_is_down_is_not_asked_again() {
    let mut w = World::new();
    let ([service, a, _b], _) = pods();
    let service = Fake::start(service, ok());
    let (server, machine_id) = holding(&w, service.addr);
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let first_only = move |_: &str| {
        if asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Reply::Answer(OK)
        } else {
            Reply::Answer(ELSEWHERE)
        }
    };
    let a = Fake::start(a, Box::new(first_only));
    link_on(&w, machine_id, &advertised(a.addr));
    assert!(call(&w, &server).starts_with("HTTP/1.1 200 OK"));
    drop(w.ledger.take());
    for _ in 0..2 {
        let reply = call(&w, &server);
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    }
    assert_eq!(
        a.seen().len(),
        2,
        "the holder that turned a call away is forgotten"
    );
    assert_eq!(service.seen().len(), 2);
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_chunked_or_oversized_call_goes_to_the_holder_once_without_asking_for_a_421() {
    let w = World::new();
    let ([service, a, _b], _) = pods();
    let (service, a) = (Fake::start(service, ok()), Fake::start(a, ok()));
    let (server, machine_id) = holding(&w, service.addr);
    link_on(&w, machine_id, &advertised(a.addr));
    let chunked = call_with(
        &w,
        &server,
        "Transfer-Encoding: chunked\r\n",
        b"5\r\nhello\r\n0\r\n\r\n",
    );
    assert!(chunked.starts_with("HTTP/1.1 200 OK"), "{chunked}");
    let oversized = call_with(&w, &server, "", &vec![b'x'; (1 << 20) + 1]);
    assert!(oversized.starts_with("HTTP/1.1 200 OK"), "{oversized}");
    let seen = a.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|r| !r.contains("X-Moor-Holder-Only")));
    assert!(service.seen().is_empty());
    server.drain(Duration::from_secs(5));
}

#[test]
fn a_box_cannot_ask_a_runner_for_a_421_itself() {
    let w = World::new();
    let ([service, _a, _b], _) = pods();
    let service = Fake::start(service, ok());
    let (server, _) = holding(&w, service.addr);
    let reply = call_with(&w, &server, "X-Moor-Holder-Only: 1\r\n", CALL.as_bytes());
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    let seen = service.seen();
    let [request] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(!request.contains("X-Moor-Holder-Only"), "{request}");
    server.drain(Duration::from_secs(5));
}
