//! The relay front passes each connection, bytes unread, to its address.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use super::super::Front;
use super::{socket_in, uid};

#[test]
fn each_connection_reaches_the_address_untouched() {
    let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
    let to = upstream.local_addr().unwrap();
    thread::spawn(move || {
        for conn in upstream.incoming() {
            let mut conn = conn.unwrap();
            let mut buf = [0_u8; 64];
            let n = conn.read(&mut buf).unwrap();
            conn.write_all(buf.get(..n).unwrap()).unwrap();
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let front = Front::relay(socket_in(&dir, uid()), to).unwrap();
    for _ in 0..2 {
        let mut client = UnixStream::connect(front.path()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let upgrade = b"GET /egress HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
        client.write_all(upgrade).unwrap();
        let mut got = vec![0_u8; upgrade.len()];
        client.read_exact(&mut got).unwrap();
        assert_eq!(got, upgrade);
    }
    front.stop();
}
