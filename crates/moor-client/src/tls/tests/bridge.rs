//! A bridge whose one side has ended waits on the other without spinning, and ends with it.

use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::ClientConnection;

use super::*;
use crate::tls::bridge;

/// Clock ticks of CPU time the thread called `name` has used; none once no thread has that name.
fn cpu_ticks(name: &str) -> Option<u64> {
    let threads = format!("/proc/{}/task", std::process::id());
    for thread in std::fs::read_dir(threads).unwrap().flatten() {
        let comm = std::fs::read_to_string(thread.path().join("comm")).unwrap_or_default();
        if comm.trim() != name {
            continue;
        }
        let stat = std::fs::read_to_string(thread.path().join("stat")).ok()?;
        // The fields after the command name start at the state, field 3; utime and stime are
        // fields 14 and 15.
        let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        return Some(utime + stime);
    }
    None
}

#[test]
fn a_bridge_whose_near_end_closed_waits_for_a_silent_peer_without_spinning() {
    let (_tmp, dir) = dir();
    let config = Arc::new(server_config(&dir));
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = tcp.local_addr().unwrap().port();
    let (release, held) = mpsc::channel::<()>();
    let peer = std::thread::spawn(move || {
        let (sock, _) = tcp.accept().unwrap();
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), sock);
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock).unwrap();
        }
        // Silent until told to hang up, as a controller that never answers.
        held.recv().ok();
    });
    let client = client_config(&files(&dir, "runner")).unwrap();
    let name = ServerName::try_from("127.0.0.1").unwrap();
    let conn = ClientConnection::new(Arc::new(client), name).unwrap();
    let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let near = bridge(StreamOwned::new(conn, sock)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let before = cpu_ticks("moor-tls").expect("the bridge thread runs");

    drop(near);
    std::thread::sleep(Duration::from_secs(1));
    let after = cpu_ticks("moor-tls").expect("the bridge waits for the peer to end");
    assert!(
        after - before < 10,
        "the bridge used {} ticks of CPU in 1 s with nothing to move",
        after - before
    );

    release.send(()).unwrap();
    peer.join().unwrap();
    let ended = (0..250).any(|_| {
        std::thread::sleep(Duration::from_millis(20));
        cpu_ticks("moor-tls").is_none()
    });
    assert!(ended, "the bridge outlived both its sides");
}
