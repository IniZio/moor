//! A stand-in for the reverse tunnel: a loopback TCP port, as the Box sees it, whose
//! connections go to a host Unix socket. Like ssh, it opens the socket per connection, so it
//! reaches a socket bound again at the same path.

use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;

/// Starts a bridge to `socket` and returns its port.
pub(crate) fn bridge(socket: &Path) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let socket = socket.to_path_buf();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(tcp) = conn else { return };
            let socket = socket.clone();
            thread::spawn(move || carry(tcp, &socket));
        }
    });
    port
}

/// Copies both ways between `tcp` and a new connection to `socket`.
fn carry(mut tcp: TcpStream, socket: &Path) {
    let Ok(mut unix) = UnixStream::connect(socket) else {
        return;
    };
    let (Ok(mut tcp_rx), Ok(mut unix_tx)) = (tcp.try_clone(), unix.try_clone()) else {
        return;
    };
    let up = thread::spawn(move || {
        let _ = io::copy(&mut tcp_rx, &mut unix_tx);
        let _ = unix_tx.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut unix, &mut tcp);
    let _ = tcp.shutdown(Shutdown::Write);
    let _ = up.join();
}
