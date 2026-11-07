//! A holder's answer when it turns a request away before reading its body.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;

use tokio::net::{TcpSocket, TcpStream};

use super::{send, try_holder, write_within, Tried};

/// A connection to a listener with a small receive buffer, itself with a small send buffer, so
/// a large write blocks until the far side reads.
async fn narrow(addr: std::net::SocketAddr) -> TcpStream {
    let socket = TcpSocket::new_v4().unwrap();
    socket.set_send_buffer_size(4096).unwrap();
    socket.connect(addr).await.unwrap()
}

/// A loopback listener whose connections get a small receive buffer.
fn narrow_listener() -> TcpListener {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    rustix::net::sockopt::set_socket_recv_buffer_size(&listener, 4096).unwrap();
    listener
}

#[test]
fn a_runner_that_keeps_the_connection_open_unread_cannot_hold_the_send() {
    let listener = narrow_listener();
    let addr = listener.local_addr().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let sent = runtime.block_on(async {
        let mut upstream = narrow(addr).await;
        tokio::time::pause();
        let body = vec![b'x'; 1 << 20];
        let write = send(&mut upstream, b"POST /mcp HTTP/1.1\r\n\r\n", &body);
        tokio::time::timeout(Duration::from_secs(600), write).await
    });
    assert_eq!(sent, Ok(false), "the send gave up on its own");
    drop(listener);
}

#[test]
fn a_runner_reading_slowly_gets_a_body_that_outlasts_the_stall_limit() {
    const BODY: usize = 256 * 1024;
    let listener = narrow_listener();
    let addr = listener.local_addr().unwrap();
    let reader = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            std::thread::sleep(Duration::from_millis(100));
            if conn.read(&mut chunk).unwrap_or(0) == 0 {
                return;
            }
        }
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let wrote = runtime.block_on(async {
        let mut upstream = narrow(addr).await;
        let wrote = write_within(&mut upstream, &vec![b'x'; BODY], Duration::from_secs(1)).await;
        drop(upstream);
        wrote
    });
    reader.join().unwrap();
    assert!(wrote, "the writes kept going while the runner read");
}

#[test]
fn a_421_sent_before_the_body_is_read_is_heard_though_the_body_write_fails() {
    let listener = narrow_listener();
    let addr = listener.local_addr().unwrap();
    let holder = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap() == 1 {
            head.extend_from_slice(&byte);
        }
        conn.write_all(b"HTTP/1.1 421 Misdirected Request\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let tried = runtime.block_on(async {
        let mut upstream = narrow(addr).await;
        let request = b"POST /mcp HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n";
        try_holder(&mut upstream, request, &vec![b'x'; 1 << 20]).await
    });
    holder.join().unwrap();
    assert!(matches!(tried, Tried::Elsewhere));
}
