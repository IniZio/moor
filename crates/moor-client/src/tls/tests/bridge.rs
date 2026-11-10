//! A bridge whose one side has ended waits on the other without spinning, and ends with it.

use std::collections::HashSet;
use std::io;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::ClientConnection;

use super::*;
use crate::tls::bridge;

/// Keeps each test's bridge thread the only new one while it looks for it.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

type Peer = StreamOwned<ServerConnection, TcpStream>;

fn threads() -> PathBuf {
    PathBuf::from(format!("/proc/{}/task", std::process::id()))
}

fn tls_threads() -> HashSet<String> {
    std::fs::read_dir(threads())
        .unwrap()
        .flatten()
        .filter(|t| {
            std::fs::read_to_string(t.path().join("comm")).is_ok_and(|c| c.trim() == "moor-tls")
        })
        .map(|t| t.file_name().to_string_lossy().into_owned())
        .collect()
}

fn cpu_ticks(tid: &str) -> Option<u64> {
    let stat = std::fs::read_to_string(threads().join(tid).join("stat"));
    // The fields after the command name start at the state, field 3; utime and stime are
    // fields 14 and 15.
    let stat = stat.ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// The near end, the bridge thread's id, and the peer `serve` runs past the handshake.
fn bridged<T: Send + 'static>(
    serve: impl FnOnce(Peer) -> T + Send + 'static,
) -> (UnixStream, String, JoinHandle<T>) {
    let (_tmp, dir) = dir();
    let config = Arc::new(server_config(&dir));
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = tcp.local_addr().unwrap().port();
    let peer = std::thread::spawn(move || {
        let (sock, _) = tcp.accept().unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), sock);
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock).unwrap();
        }
        serve(tls)
    });
    let client = client_config(&files(&dir, "runner")).unwrap();
    let name = ServerName::try_from("127.0.0.1").unwrap();
    let conn = ClientConnection::new(Arc::new(client), name).unwrap();
    let sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let before = tls_threads();
    let near = bridge(StreamOwned::new(conn, sock)).unwrap();
    let mut new = Vec::new();
    for _ in 0..5000 {
        new = tls_threads().difference(&before).cloned().collect();
        if !new.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let [tid] = <[String; 1]>::try_from(new).expect("one new bridge thread");
    (near, tid, peer)
}

fn read_to_close(tls: &mut Peer) -> io::Result<Vec<u8>> {
    let mut got = Vec::new();
    tls.read_to_end(&mut got)?;
    Ok(got)
}

fn assert_quiet(tid: &str, what: &str) {
    let before = cpu_ticks(tid).expect("the bridge thread runs");
    std::thread::sleep(Duration::from_secs(1));
    let after = cpu_ticks(tid).expect("the bridge waits for its other side");
    assert!(
        after - before < 3,
        "the bridge used {} ticks of CPU in 1 s while {what}",
        after - before
    );
}

fn assert_ends(tid: &str) {
    let ended = (0..250).any(|_| {
        std::thread::sleep(Duration::from_millis(20));
        cpu_ticks(tid).is_none()
    });
    assert!(ended, "the bridge outlived both its sides");
}

#[test]
fn a_bridge_whose_near_end_closed_waits_for_a_silent_peer_without_spinning() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (release, held) = mpsc::channel::<()>();
    let (near, tid, peer) = bridged(move |mut tls| {
        held.recv().ok();
        let got = read_to_close(&mut tls)?;
        tls.conn.send_close_notify();
        tls.flush()?;
        Ok::<_, io::Error>(got)
    });
    std::thread::sleep(Duration::from_millis(300));

    drop(near);
    assert_quiet(&tid, "the peer stays silent");

    release.send(()).unwrap();
    assert_ends(&tid);
    let got = peer
        .join()
        .unwrap()
        .expect("the bridge closed its TLS cleanly");
    assert!(got.is_empty(), "the peer got {} bytes", got.len());
}

/// The peer sends `sent`, ends its writes with or without a `close_notify`, and reads to
/// the client's `close_notify`; the near end reads everything, then stays open and silent.
fn peer_ends_first(close_notify: bool) {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let sent: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
    let to_send = sent.clone();
    let (mut near, tid, peer) = bridged(move |mut tls| {
        tls.write_all(&to_send)?;
        if close_notify {
            tls.conn.send_close_notify();
        }
        tls.flush()?;
        tls.sock.shutdown(Shutdown::Write)?;
        read_to_close(&mut tls)
    });

    near.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut got = Vec::new();
    near.read_to_end(&mut got)
        .expect("the peer's end reaches the near end");
    assert!(
        got == sent,
        "the near end got {} of {} bytes",
        got.len(),
        sent.len()
    );
    assert_quiet(&tid, "the near end stays open and silent");

    drop(near);
    assert_ends(&tid);
    let back = peer
        .join()
        .unwrap()
        .expect("the bridge closed its TLS cleanly");
    assert!(back.is_empty(), "the peer got {} bytes", back.len());
}

#[test]
fn a_bridge_whose_peer_closed_cleanly_waits_for_a_silent_near_end_without_spinning() {
    peer_ends_first(true);
}

#[test]
fn a_bridge_whose_peer_hung_up_waits_for_a_silent_near_end_without_spinning() {
    peer_ends_first(false);
}
