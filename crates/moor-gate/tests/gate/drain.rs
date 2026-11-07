//! A gate that drains: it stops accepting, lets requests in flight finish, and ends the
//! streams that stay open on purpose within [`moor_gate::STREAM_DRAIN_SECS`].

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moor_gate::{Gate, Verifier};
use moor_testkit::oidc::Issuer;
use serde_json::json;

use super::listen::{serve_controller as serve_watches, token, wait_live, Fake};
use super::{claims, connect, recv, send, serve_controller, sign, World, CLIENT_ID};

/// A gate that drains when the returned socket's peer is written to, and the thread serving it.
fn world_draining() -> (World, UnixStream, std::thread::JoinHandle<()>) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    world_draining_on(socket, seen, dir, false)
}

/// A gate like [`world_draining`] over the controller at `socket`, serving `/mcp` when `mcp`.
fn world_draining_on(
    socket: std::path::PathBuf,
    seen: mpsc::Receiver<String>,
    dir: tempfile::TempDir,
    mcp: bool,
) -> (World, UnixStream, std::thread::JoinHandle<()>) {
    let (world, trigger, start) = world_unserved_on(socket, seen, dir, mcp);
    (world, trigger, start())
}

/// A bound gate that has not started serving, so connections made to it queue in the kernel.
/// Calling the returned function starts serving.
fn world_unserved_on(
    socket: std::path::PathBuf,
    seen: mpsc::Receiver<String>,
    dir: tempfile::TempDir,
    mcp: bool,
) -> (World, UnixStream, impl FnOnce() -> JoinHandle<()>) {
    let Issuer {
        base: issuer,
        key,
        userinfo,
        userinfo_calls,
        ..
    } = Issuer::start(CLIENT_ID);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = listener.local_addr().unwrap().to_string();
    let mcp = mcp.then(|| {
        moor_gate::Mcp::new(
            &format!("http://{gate}/mcp"),
            moor_client::Client::new(&socket),
        )
        .unwrap()
    });
    let config = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
        mcp,
        verifier: Verifier::new(issuer.clone(), CLIENT_ID.to_owned(), None),
    });
    let (wake, trigger) = UnixStream::pair().unwrap();
    let start = move || {
        std::thread::spawn(move || {
            moor_gate::serve_until(
                listener,
                &config,
                None,
                &wake,
                std::time::Duration::from_secs(30),
            )
            .unwrap();
        })
    };
    let world = World {
        issuer,
        gate,
        key,
        userinfo,
        userinfo_calls,
        seen,
        _dir: dir,
    };
    (world, trigger, start)
}

/// A draining gate with `/mcp` over the fake controller, the trigger that starts its drain,
/// and the thread serving it.
fn mcp_world_draining() -> (World, UnixStream, JoinHandle<()>, Arc<Mutex<Fake>>) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let fake = serve_watches(&socket);
    let (_unused, rx) = mpsc::channel();
    let (world, trigger, served) = world_draining_on(socket, rx, dir, true);
    (world, trigger, served, fake)
}

/// How long after `began` the gate served by `served` returned, or `None` when it did not
/// within `bound`.
fn exited_within(served: &JoinHandle<()>, began: Instant, bound: Duration) -> Option<Duration> {
    while began.elapsed() < bound {
        if served.is_finished() {
            return Some(began.elapsed());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Waits until the gate at `addr` no longer accepts connections.
fn wait_refused(addr: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while TcpStream::connect(addr).is_ok() {
        assert!(std::time::Instant::now() < deadline, "still accepting");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_draining_gate_finishes_a_short_request_and_refuses_new_connections() {
    let (w, mut trigger, served) = world_draining();
    let mut stream = TcpStream::connect(&w.gate).unwrap();
    write!(
        stream,
        "POST /machines/enroll HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{{"
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    trigger.write_all(b"x").unwrap();
    wait_refused(&w.gate);
    assert!(
        !served.is_finished(),
        "the drain must wait for the open request"
    );

    stream.write_all(b"}").unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    served.join().unwrap();
}

#[test]
fn a_draining_gate_serves_a_connection_queued_before_the_drain() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let seen = serve_controller(&socket);
    let (w, mut trigger, start) = world_unserved_on(socket, seen, dir, false);
    let mut stream = TcpStream::connect(&w.gate).unwrap();
    write!(
        stream,
        "POST /machines/enroll HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    )
    .unwrap();
    trigger.write_all(b"x").unwrap();
    let served = start();

    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    served.join().unwrap();
}

#[test]
fn a_draining_gate_closes_an_open_front_session_within_the_stream_bound() {
    let (w, mut trigger, served) = world_draining();
    let mut ws = connect(&w, Some(&sign(&w.key, &claims(&w)))).unwrap();
    send(&mut ws, &json!({"frame": "hello", "protocol": 15}));
    assert_eq!(recv(&mut ws), json!({"frame": "hello", "protocol": 15}));

    let began = std::time::Instant::now();
    trigger.write_all(b"x").unwrap();
    wait_refused(&w.gate);
    assert!(ws.read().is_err() || ws.read().is_err());
    let bound = std::time::Duration::from_secs(moor_gate::STREAM_DRAIN_SECS);
    assert!(
        began.elapsed() < bound * 2,
        "closed after {:?}",
        began.elapsed()
    );
    served.join().unwrap();
}

#[test]
fn a_draining_gate_ends_an_open_listen_cleanly_and_exits_at_the_stream_bound() {
    let (world, mut trigger, served, fake) = mcp_world_draining();
    let token = token(&world, "moor:read");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "subscriptions/listen",
        "params": {"notifications": {"resourcesListChanged": true}, "_meta": meta}});
    let res = agent
        .post(format!("http://{}/mcp", world.gate))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .send(body.to_string())
        .unwrap();
    let (ended_tx, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = res.into_body().into_reader().read_to_string(&mut text);
        let _ = ended_tx.send((read.map_err(|e| e.to_string()), text));
    });
    wait_live(&fake);

    let began = Instant::now();
    trigger.write_all(b"x").unwrap();
    let bound = Duration::from_secs(moor_gate::STREAM_DRAIN_SECS + 1);
    let exited = exited_within(&served, began, bound + Duration::from_secs(5));
    assert!(
        exited.is_some_and(|after| after < bound),
        "the gate exited after {exited:?} with a listen open; the bound is {bound:?}"
    );
    let (read, text) = ended.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        read.is_ok(),
        "the listen ended with {read:?} after {text:?}"
    );
    assert!(
        text.contains("notifications/subscriptions/acknowledged"),
        "{text}"
    );
}

#[test]
fn a_draining_gate_closes_an_idle_mcp_connection_without_waiting_out_the_bound() {
    let (world, mut trigger, served, _fake) = mcp_world_draining();
    let token = token(&world, "moor:read");
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list",
        "params": {"_meta": meta}})
    .to_string();
    let mut conn = TcpStream::connect(&world.gate).unwrap();
    write!(
        conn,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAuthorization: Bearer {token}\r\nMCP-Protocol-Version: 2026-07-28\r\nMcp-Method: resources/list\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut answer = Vec::new();
    let mut buf = [0_u8; 4096];
    while !String::from_utf8_lossy(&answer).contains("moor://box/repo/b") {
        let n = conn.read(&mut buf).unwrap();
        assert!(n > 0, "{}", String::from_utf8_lossy(&answer));
        answer.extend_from_slice(&buf[..n]);
    }

    let began = Instant::now();
    trigger.write_all(b"x").unwrap();
    let bound = Duration::from_secs(1);
    let exited = exited_within(&served, began, bound + Duration::from_secs(10));
    assert!(
        exited.is_some_and(|after| after < bound),
        "the gate exited after {exited:?} with an idle /mcp connection open"
    );
    assert_eq!(
        conn.read(&mut buf).unwrap(),
        0,
        "the gate closed the connection"
    );
}
