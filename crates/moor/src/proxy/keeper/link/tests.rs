#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;

use moor_client::forward::{Carried, Forward, Forwarding};
use moor_client::link::reach::{Dial, Hello, Keyed};
use moor_core::cache::CachePlan;
use moor_core::name::BoxName;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxHandle, BoxId, ProviderId};

use moor_client::link::reach::Runner;

use super::{keep, MachineLink, Via};
use crate::testing::driver::{eventually, FakeDriver, LOCAL_PORT_OFFSET};

const KEY: &str = "machine-service-key";

fn boxed(fake: &FakeDriver) -> BoxHandle {
    fake.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").expect("name"),
        class: None,
        nonce: "n0".to_owned(),
        cache: CachePlan::none(),
    })
    .expect("acquired")
    .handle
}

/// A runner that answers each forward request with `answer`, holds the connection, and sends
/// every request it read plus its held connection back.
fn runner(socket: &Path, answer: Forwarding) -> mpsc::Receiver<(Forward, UnixStream)> {
    let listener = UnixListener::bind(socket).expect("bind");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = conn.expect("accept");
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).expect("read");
            let req: Forward = serde_json::from_str(&line).expect("forward request");
            let mut out = serde_json::to_vec(&answer).expect("answer");
            out.push(b'\n');
            conn.write_all(&out).expect("write");
            if tx.send((req, conn)).is_err() {
                return;
            }
        }
    });
    rx
}

/// The runner on the host of state dir `dir`, and its forward socket.
fn local_runner(dir: &Path) -> (Runner, PathBuf) {
    let socket = crate::runner_dir::of(dir).join(moor_client::forward::SOCKET);
    std::fs::create_dir_all(socket.parent().expect("runner dir")).expect("runner dir");
    (Runner::Local(socket.with_file_name("runner.sock")), socket)
}

fn via(runner: &Runner) -> Via<'_> {
    Via {
        named: runner,
        dialed: runner,
        box_id: 7,
        machine_id: 3,
    }
}

#[test]
fn opens_a_tunnel_per_port_then_asks_the_runner_to_serve_them() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, socket) = local_runner(dir.path());
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let link = MachineLink::open(&driver, &h, &via(&at), &[8081, 3001]).expect("open");
    assert!(link.alive());
    let ports: Vec<u16> = driver.tunnels().iter().map(|t| t.box_port).collect();
    assert_eq!(ports, [8081, 3001]);
    let (req, _conn) = seen.recv().expect("runner asked");
    assert_eq!((req.box_id, req.machine_id), (7, 3));
    assert_eq!(req.ports, [8081, 3001]);
}

#[test]
fn a_dead_link_is_reopened_and_close_ends_every_tunnel() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, socket) = local_runner(dir.path());
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let open = || MachineLink::open(&driver, &h, &via(&at), &[8081]);
    let mut link = Some(open().expect("open"));
    let first = driver.tunnels().len();
    keep(&mut link, Some(&at), open).expect("alive link kept");
    assert_eq!(driver.tunnels().len(), first, "a live link is left alone");
    let (_, held) = seen.recv().expect("runner asked");
    held.shutdown(Shutdown::Both).expect("runner lets go");
    assert!(
        eventually(|| !link.as_ref().is_some_and(MachineLink::alive)),
        "the closed carrier was never noticed"
    );
    keep(&mut link, Some(&at), open).expect("reopened");
    assert_eq!(
        driver.tunnels().len(),
        first + 1,
        "the link the runner let go was opened again"
    );
    drop(link);
    assert!(
        driver.dropped_tunnels().contains(&50_001),
        "close ends the tunnel"
    );
}

#[test]
fn a_live_link_whose_row_names_another_runner_is_reopened() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, socket) = local_runner(dir.path());
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let open = || MachineLink::open(&driver, &h, &via(&at), &[8081]);
    let mut link = Some(open().expect("open"));
    let (_, _held) = seen.recv().expect("runner asked");
    let moved = Runner::Remote {
        addr: "10.0.0.8:7701".to_owned(),
        key: "k".to_owned(),
    };
    keep(&mut link, None, open).expect("an unknown runner keeps the link");
    assert_eq!(driver.tunnels().len(), 1);
    keep(&mut link, Some(&moved), open).expect("reopened");
    assert_eq!(
        driver.tunnels().len(),
        2,
        "a moved row opens the forwards again"
    );
}

#[test]
fn a_name_that_resolves_to_another_address_each_tick_keeps_the_link() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, socket) = local_runner(dir.path());
    let seen = runner(&socket, Forwarding::Up);
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let named = Runner::Remote {
        addr: "runner.moor.internal:7701".to_owned(),
        key: "k".to_owned(),
    };
    let address = |ip: &str| Runner::Remote {
        addr: format!("{ip}:7701"),
        key: "k".to_owned(),
    };
    let via = Via {
        named: &named,
        dialed: &at,
        box_id: 7,
        machine_id: 3,
    };
    let mut link = Some(MachineLink::open(&driver, &h, &via, &[8081]).expect("open"));
    let (_, _held) = seen.recv().expect("runner asked");
    for ip in ["10.0.0.7", "10.0.0.8", "10.0.0.7", "10.0.0.8"] {
        let dialed = address(ip);
        let via = Via {
            dialed: &dialed,
            ..via
        };
        let open = || MachineLink::open(&driver, &h, &via, &[8081]);
        keep(&mut link, Some(&named), open).expect("kept");
    }
    assert_eq!(
        driver.tunnels().len(),
        1,
        "the link row's name, not an address, is compared"
    );
}

#[test]
fn a_failed_tunnel_names_its_port() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, _) = local_runner(dir.path());
    let driver = FakeDriver::new();
    let h = boxed(&driver);
    let err = MachineLink::open(&driver, &h, &via(&at), &[8081])
        .err()
        .expect("no tunnel support");
    assert_eq!(err.port, Some(8081));
}

#[test]
fn a_refused_forward_carries_the_runner_reason() {
    let dir = tempfile::tempdir().expect("dir");
    let (at, socket) = local_runner(dir.path());
    let _seen = runner(
        &socket,
        Forwarding::Refused {
            reason: "mini is not linked".to_owned(),
        },
    );
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let err = MachineLink::open(&driver, &h, &via(&at), &[8081])
        .err()
        .expect("refused");
    assert_eq!((err.port, err.error.as_str()), (None, "mini is not linked"));
}

fn read_line<T: serde::de::DeserializeOwned>(conn: &mut impl Read) -> T {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while conn.read(&mut byte).expect("read") == 1 && byte[0] != b'\n' {
        line.push(byte[0]);
    }
    serde_json::from_slice(&line).expect("line")
}

fn write_line(conn: &mut impl Write, value: &impl serde::Serialize) {
    let mut out = serde_json::to_vec(value).expect("line");
    out.push(b'\n');
    conn.write_all(&out).expect("write");
}

/// Echoes every connection on a loopback port above the fake driver's tunnel offset; the Box
/// port whose tunnel lands there.
fn box_port_tunnel() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let host = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            thread::spawn(move || {
                let mut out = conn.try_clone().expect("clone");
                let _ = std::io::copy(&mut &conn, &mut out);
            });
        }
    });
    host.checked_sub(LOCAL_PORT_OFFSET)
        .expect("an ephemeral port above the offset")
}

#[test]
fn a_named_connection_is_dialed_back_across_the_network_and_piped_to_its_tunnel() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let at = Runner::Remote {
        addr: listener.local_addr().expect("addr").to_string(),
        key: KEY.to_owned(),
    };
    let box_port = box_port_tunnel();
    let keyed = Keyed {
        v: 1,
        key: KEY.to_owned(),
    };

    let runner = thread::spawn(move || {
        let (mut carrier, _) = listener.accept().expect("carrier");
        assert_eq!(read_line::<Hello>(&mut carrier), Hello::Forwards(keyed));
        let req: Forward = read_line(&mut carrier);
        write_line(&mut carrier, &Forwarding::Up);
        write_line(
            &mut carrier,
            &Carried::Conn {
                n: 9,
                port: box_port,
            },
        );
        let (mut back, _) = listener.accept().expect("dial back");
        let hello: Hello = read_line(&mut back);
        back.write_all(b"ping").expect("machine bytes");
        let mut got = [0u8; 4];
        back.read_exact(&mut got).expect("box bytes");
        (req, hello, got, carrier)
    });
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let link = MachineLink::open(&driver, &h, &via(&at), &[box_port]).expect("open");
    let (req, hello, got, _carrier) = runner.join().expect("runner");
    assert_eq!(req.ports, [box_port], "the forward names Box ports only");
    let dial = Dial {
        v: 1,
        key: KEY.to_owned(),
        n: 9,
    };
    assert_eq!(hello, Hello::Conn(dial));
    assert_eq!(
        &got, b"ping",
        "the bytes went through the Box port's tunnel and back"
    );
    assert!(link.alive());
}

/// A fake runner across the network, and the link the keeper opens on it for one Box port.
fn remote_link(
    serve: impl FnOnce(TcpListener, u16) + Send + 'static,
) -> (MachineLink, FakeDriver, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let at = Runner::Remote {
        addr: listener.local_addr().expect("addr").to_string(),
        key: KEY.to_owned(),
    };
    let box_port = box_port_tunnel();
    let runner = thread::spawn(move || serve(listener, box_port));
    let driver = FakeDriver::new().reverse_tunnels();
    let h = boxed(&driver);
    let link = MachineLink::open(&driver, &h, &via(&at), &[box_port]).expect("open");
    (link, driver, runner)
}

/// Answers the forward on `listener` and sends `lines` on the carrier; the carrier.
fn carry(listener: &TcpListener, lines: &[Carried]) -> std::net::TcpStream {
    let (mut carrier, _) = listener.accept().expect("carrier");
    let _: Hello = read_line(&mut carrier);
    let _: Forward = read_line(&mut carrier);
    write_line(&mut carrier, &Forwarding::Up);
    for line in lines {
        write_line(&mut carrier, line);
    }
    carrier
}

#[test]
fn a_conn_line_for_a_port_the_forward_did_not_list_gets_no_dial_back() {
    let (seen_tx, seen) = mpsc::channel();
    let (_link, _driver, runner) = remote_link(move |listener, box_port| {
        let unlisted = Carried::Conn { n: 1, port: 1 };
        let listed = Carried::Conn {
            n: 2,
            port: box_port,
        };
        let _carrier = carry(&listener, &[unlisted, listed]);
        let (mut back, _) = listener.accept().expect("dial back");
        seen_tx.send(read_line::<Hello>(&mut back)).expect("send");
    });
    let dial = Dial {
        v: 1,
        key: KEY.to_owned(),
        n: 2,
    };
    assert_eq!(
        seen.recv().expect("dialed"),
        Hello::Conn(dial),
        "only the listed port"
    );
    runner.join().expect("runner");
}

#[test]
fn ending_the_link_ends_its_pipes() {
    let (back_tx, back) = mpsc::channel();
    let (link, _driver, runner) = remote_link(move |listener, box_port| {
        let carrier = carry(
            &listener,
            &[Carried::Conn {
                n: 3,
                port: box_port,
            }],
        );
        let (mut back, _) = listener.accept().expect("dial back");
        let _: Hello = read_line(&mut back);
        back.write_all(b"ping").expect("machine bytes");
        let mut got = [0u8; 4];
        back.read_exact(&mut got).expect("box bytes");
        back_tx.send((back, carrier)).expect("send");
    });
    let (mut back, _carrier) = back.recv().expect("piped");
    runner.join().expect("runner");
    let pipes = Arc::clone(&link.pipes);
    assert_eq!(pipes.live(), 1);
    drop(link);
    back.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("timeout");
    assert_eq!(back.read(&mut [0u8; 4]).expect("closed, not timed out"), 0);
    assert!(eventually(|| pipes.live() == 0), "every pipe ended");
}

#[test]
fn pipes_beyond_the_cap_are_refused_and_reported_once() {
    let pipes = super::Pipes::default();
    for _ in 0..super::MAX_PIPES {
        assert!(pipes.reserve().is_some());
    }
    assert_eq!(pipes.reserve(), None);
    pipes.note_over(8081);
    pipes.note_over(3001);
    assert_eq!(pipes.take_over(), Some(8081));
    pipes.note_over(8081);
    assert_eq!(pipes.take_over(), None, "reported once per link");
    pipes.release(0);
    assert!(pipes.reserve().is_some(), "a freed pipe makes room");
    pipes.close();
    pipes.release(1);
    assert_eq!(pipes.reserve(), None, "an ended link starts no pipe");
}
