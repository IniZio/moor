//! Socket Mode through a CONNECT proxy, as a front the controller runs must reach Slack.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

use super::super::via;

/// A CONNECT proxy on loopback that answers one request with `answer`, then sends `after`;
/// returns it as a proxy and the request head it saw.
fn fake_proxy(answer: &'static str, after: &'static [u8]) -> (ureq::Proxy, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            conn.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        conn.write_all(answer.as_bytes()).unwrap();
        conn.write_all(after).unwrap();
        String::from_utf8(head).unwrap()
    });
    (
        ureq::Proxy::new(&format!("http://127.0.0.1:{port}")).unwrap(),
        handle,
    )
}

#[test]
fn socket_mode_opens_its_connection_through_the_proxy_and_reads_nothing_of_the_tunnel() {
    let (proxy, saw) = fake_proxy("HTTP/1.1 200 Connection Established\r\n\r\n", b"tunnel");
    let mut tcp = via(
        Some(proxy),
        "wss://wss-primary.slack.com/link",
        "wss-primary.slack.com",
        443,
    )
    .unwrap();
    let mut first = [0_u8; 6];
    tcp.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"tunnel");
    assert!(saw
        .join()
        .unwrap()
        .starts_with("CONNECT wss-primary.slack.com:443 HTTP/1.1\r\n"));
}

#[test]
fn a_tunnel_the_proxy_refuses_is_a_connect_error_naming_its_status() {
    let (proxy, _saw) = fake_proxy("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n", b"");
    let e = via(
        Some(proxy),
        "wss://wss-primary.slack.com/link",
        "wss-primary.slack.com",
        443,
    )
    .err()
    .unwrap();
    assert!(e.to_string().contains("403 Forbidden"), "{e}");
}
