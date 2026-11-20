//! The relay against a stand-in Proxy on a Unix socket.
#![allow(clippy::unwrap_used, reason = "tests")]

use std::path::Path;
use std::time::Duration;

use moor_egress_forward::relay;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream, UnixListener};

/// A relay on a loopback port in front of `socket`; returns the port.
async fn relay_to(socket: &Path) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(relay(listener, socket.to_path_buf()));
    port
}

/// A stand-in Proxy that answers each connection with its input upper-cased, until the client
/// closes its side.
fn upper_proxy(socket: &Path) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut conn, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0_u8; 1024];
                while let Ok(n) = conn.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let up = buf.get(..n).unwrap().to_ascii_uppercase();
                    if conn.write_all(&up).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
}

#[tokio::test]
async fn bytes_cross_both_ways_and_each_connection_has_its_own_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("egress.sock");
    upper_proxy(&socket);
    let port = relay_to(&socket).await;

    let mut a = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut b = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    a.write_all(b"alpha").await.unwrap();
    b.write_all(b"beta").await.unwrap();
    let (mut got_a, mut got_b) = ([0_u8; 5], [0_u8; 4]);
    a.read_exact(&mut got_a).await.unwrap();
    b.read_exact(&mut got_b).await.unwrap();
    assert_eq!((&got_a, &got_b), (b"ALPHA", b"BETA"));
}

#[tokio::test]
async fn a_client_close_reaches_the_proxy_and_a_proxy_close_reaches_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("egress.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let port = relay_to(&socket).await;
    let (seen_eof_tx, seen_eof_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut rest = Vec::new();
        conn.read_to_end(&mut rest).await.unwrap();
        seen_eof_tx.send(rest).unwrap();
        let (mut conn, _) = listener.accept().await.unwrap();
        conn.write_all(b"bye").await.unwrap();
    });

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(b"last words").await.unwrap();
    client.shutdown().await.unwrap();
    let rest = tokio::time::timeout(Duration::from_secs(5), seen_eof_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rest, b"last words");

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut all = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut all))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(all, b"bye");
}

#[tokio::test]
async fn a_missing_proxy_closes_the_client_and_the_relay_keeps_serving() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("egress.sock");
    let port = relay_to(&socket).await;

    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut all = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut all))
        .await
        .unwrap()
        .unwrap_or_default();
    assert!(all.is_empty());

    upper_proxy(&socket);
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(b"up").await.unwrap();
    let mut got = [0_u8; 2];
    client.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"UP");
}
