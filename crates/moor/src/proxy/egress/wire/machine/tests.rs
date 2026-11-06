//! A holder's answer when it turns a request away before reading its body.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener};

use tokio::net::TcpSocket;

use super::{try_holder, Tried};

#[test]
fn a_421_sent_before_the_body_is_read_is_heard_though_the_body_write_fails() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    rustix::net::sockopt::set_socket_recv_buffer_size(&listener, 4096).unwrap();
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
        let socket = TcpSocket::new_v4().unwrap();
        socket.set_send_buffer_size(4096).unwrap();
        let mut upstream = socket.connect(addr).await.unwrap();
        let request = b"POST /mcp HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n";
        try_holder(&mut upstream, request, &vec![b'x'; 1 << 20]).await
    });
    holder.join().unwrap();
    assert!(matches!(tried, Tried::Elsewhere));
}
