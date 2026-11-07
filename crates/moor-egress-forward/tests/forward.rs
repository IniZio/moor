//! The forwarder against a stand-in shared Proxy on loopback.
#![allow(
    clippy::unwrap_used,
    clippy::result_large_err,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use moor_egress_forward::{next_accept_pause, serve, Config, Upstream};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::RootCertStore;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

/// What the stand-in Proxy saw in an upgrade.
#[derive(Debug)]
struct Seen {
    path: String,
    auth: Option<String>,
    target: Option<String>,
    plain: Option<String>,
}

/// Takes one upgrade on `stream`, reports it, and echoes binary messages until the client closes.
async fn echo<S: AsyncRead + AsyncWrite + Unpin>(stream: S, seen: mpsc::UnboundedSender<Seen>) {
    let header = |req: &Request, name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let record = |req: &Request, res: Response| {
        drop(seen.send(Seen {
            path: req.uri().path().to_owned(),
            auth: header(req, "authorization"),
            target: header(req, "x-moor-target"),
            plain: header(req, "x-moor-plain"),
        }));
        Ok(res)
    };
    let mut ws = tokio_tungstenite::accept_hdr_async(stream, record)
        .await
        .unwrap();
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Binary(b) => {
                assert!(b.len() <= 64 * 1024, "a {} byte message", b.len());
                ws.send(Message::Binary(b)).await.unwrap();
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

/// A plain `ws://` stand-in Proxy that echoes.
async fn echo_proxy() -> (SocketAddr, mpsc::UnboundedReceiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(echo(s, tx.clone()));
        }
    });
    (addr, rx)
}

/// A stand-in Proxy that answers every request with `reply` and closes.
async fn refusing_proxy(reply: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            drop(s.read(&mut buf).await);
            s.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    addr
}

/// A forwarder sending to `url` with token `tok`, trusting `roots`.
async fn forwarder(url: String, roots: RootCertStore) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream = Upstream::new(url, "tok", roots)
        .unwrap()
        .with_patience(Duration::from_millis(500));
    tokio::spawn(serve(listener, Arc::new(upstream)));
    addr
}

/// Sends `request` to the forwarder and returns the connection and the reply head.
async fn send(fwd: SocketAddr, request: &[u8]) -> (TcpStream, String) {
    let mut c = TcpStream::connect(fwd).await.unwrap();
    c.write_all(request).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        if c.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        head.extend_from_slice(&byte);
    }
    (c, String::from_utf8(head).unwrap())
}

/// Reads exactly `n` bytes.
async fn read_n(c: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    tokio::time::timeout(Duration::from_secs(5), c.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    buf
}

#[tokio::test]
async fn a_connect_becomes_a_websocket_with_the_token_and_target() {
    let (proxy, mut seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (mut c, head) = send(
        fwd,
        b"CONNECT api.github.com:443 HTTP/1.1\r\nHost: api.github.com:443\r\n\r\n",
    )
    .await;
    assert_eq!(head, "HTTP/1.1 200 Connection established\r\n\r\n");
    c.write_all(b"ping through").await.unwrap();
    assert_eq!(read_n(&mut c, 12).await, b"ping through");
    let seen = seen.recv().await.unwrap();
    assert_eq!(seen.path, "/egress");
    assert_eq!(seen.auth.as_deref(), Some("Bearer tok"));
    assert_eq!(seen.target.as_deref(), Some("api.github.com:443"));
}

#[tokio::test]
async fn bytes_sent_with_the_connect_head_reach_the_tunnel_first() {
    let (proxy, _seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (mut c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\nearly").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    c.write_all(b"-late").await.unwrap();
    assert_eq!(read_n(&mut c, 10).await, b"early-late");
}

#[tokio::test]
async fn a_large_transfer_arrives_whole() {
    let (proxy, _seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (c, _) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    let (mut rd, mut wr) = c.into_split();
    let body: Vec<u8> = (0..4_000_000u32).map(|i| (i % 251) as u8).collect();
    let sent = body.clone();
    let writer = tokio::spawn(async move { wr.write_all(&sent).await.unwrap() });
    let mut got = vec![0u8; body.len()];
    tokio::time::timeout(Duration::from_secs(20), rd.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    writer.await.unwrap();
    assert!(got == body, "the echoed bytes differ");
}

#[tokio::test]
async fn a_refusal_from_the_proxy_is_the_connects_answer() {
    let proxy = refusing_proxy(
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 18\r\nConnection: close\r\n\r\nhost not permitted",
    )
    .await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (mut c, head) = send(fwd, b"CONNECT evil.example:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{head}");
    assert!(head.contains("Content-Length: 18\r\n"), "{head}");
    assert_eq!(read_n(&mut c, 18).await, b"host not permitted");
}

#[tokio::test]
async fn an_unknown_token_is_401() {
    let proxy = refusing_proxy("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (_c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 401 Unauthorized\r\n"), "{head}");
}

#[tokio::test]
async fn an_unreachable_proxy_is_502() {
    let hangup = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = hangup.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = hangup.accept().await {
            drop(s);
        }
    });
    let fwd = forwarder(format!("ws://{addr}/egress"), RootCertStore::empty()).await;
    let (_c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{head}");
}

#[tokio::test]
async fn a_proxy_back_after_a_controller_restart_gap_takes_the_connect() {
    let gone = moor_testkit::LockedPort::reserve();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, gone.port()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fwd = listener.local_addr().unwrap();
    let upstream = Upstream::new(format!("ws://{addr}/egress"), "tok", RootCertStore::empty());
    tokio::spawn(serve(listener, Arc::new(upstream.unwrap())));
    let (tx, _seen) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let listener = TcpListener::bind(addr).await.unwrap();
        let (s, _) = listener.accept().await.unwrap();
        echo(s, tx).await;
    });
    let (mut c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    c.write_all(b"back").await.unwrap();
    assert_eq!(read_n(&mut c, 4).await, b"back");
}

#[tokio::test]
async fn a_proxy_back_within_the_retries_takes_the_connect() {
    let gone = moor_testkit::LockedPort::reserve();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, gone.port()));
    let fwd = forwarder(format!("ws://{addr}/egress"), RootCertStore::empty()).await;
    let (tx, _seen) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let listener = TcpListener::bind(addr).await.unwrap();
        let (s, _) = listener.accept().await.unwrap();
        echo(s, tx).await;
    });
    let (mut c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    c.write_all(b"back").await.unwrap();
    assert_eq!(read_n(&mut c, 4).await, b"back");
}

#[tokio::test]
async fn a_plain_request_goes_whole_into_a_plain_tunnel_and_its_reply_comes_back_as_sent() {
    let (proxy, mut seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let request =
        b"GET http://machine.moor.internal/mcp HTTP/1.1\r\nHost: machine.moor.internal\r\n\r\n";
    let mut c = TcpStream::connect(fwd).await.unwrap();
    c.write_all(request).await.unwrap();
    assert_eq!(read_n(&mut c, request.len()).await, request);
    let seen = seen.recv().await.unwrap();
    assert_eq!(seen.auth.as_deref(), Some("Bearer tok"));
    assert_eq!(seen.target.as_deref(), Some("machine.moor.internal:80"));
    assert_eq!(seen.plain.as_deref(), Some("1"));
}

#[tokio::test]
async fn a_plain_request_keeps_its_port_and_a_connect_is_not_plain() {
    let (proxy, mut seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let request = b"POST http://h:8080?q=1 HTTP/1.1\r\nContent-Length: 0\r\n\r\n";
    let mut c = TcpStream::connect(fwd).await.unwrap();
    c.write_all(request).await.unwrap();
    assert_eq!(read_n(&mut c, request.len()).await, request);
    assert_eq!(seen.recv().await.unwrap().target.as_deref(), Some("h:8080"));
    let (_c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(seen.recv().await.unwrap().plain, None);
}

#[tokio::test]
async fn only_connect_and_absolute_http_are_taken() {
    let (proxy, _seen) = echo_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    for request in [
        &b"GET / HTTP/1.1\r\nHost: h\r\n\r\n"[..],
        b"GET https://h/ HTTP/1.1\r\nHost: h\r\n\r\n",
    ] {
        let (_c, head) = send(fwd, request).await;
        assert!(head.starts_with("HTTP/1.1 405 "), "{head}");
    }
    let (_c, head) = send(fwd, b"CONNECT nohost HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 400 "), "{head}");
    let (_c, head) = send(fwd, b"GET http:///x HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 400 "), "{head}");
}

#[tokio::test]
async fn the_tunnel_ends_when_the_proxy_closes_it() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        ws.send(Message::binary(b"bye".to_vec())).await.unwrap();
        ws.close(None).await.unwrap();
    });
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (mut c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rest, b"bye");
}

#[tokio::test]
async fn a_wss_upstream_is_verified_against_the_given_roots() {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = CertificateDer::from(key.cert.der().to_vec());
    let pkcs8 = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()));
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.clone()], pkcs8)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, mut seen) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            if let Ok(tls) = acceptor.accept(s).await {
                tokio::spawn(echo(tls, tx.clone()));
            }
        }
    });
    let url = format!("wss://localhost:{port}/egress");

    let untrusted = forwarder(url.clone(), RootCertStore::empty()).await;
    let (_c, head) = send(untrusted, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 502 "), "{head}");

    let mut roots = RootCertStore::empty();
    roots.add(cert).unwrap();
    let trusted = forwarder(url, roots).await;
    let (mut c, head) = send(trusted, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    c.write_all(b"tls").await.unwrap();
    assert_eq!(read_n(&mut c, 3).await, b"tls");
    assert_eq!(
        seen.recv().await.unwrap().auth.as_deref(),
        Some("Bearer tok")
    );
}

/// A forwarder with no token of its own, on a stand-in Proxy at `proxy`.
async fn lease_forwarder(proxy: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let upstream =
        Upstream::passing_leases(format!("ws://{proxy}/egress"), RootCertStore::empty()).unwrap();
    tokio::spawn(serve(listener, Arc::new(upstream)));
    addr
}

#[tokio::test]
async fn with_no_token_each_clients_proxy_password_goes_on_as_authorization() {
    let (proxy, mut seen) = echo_proxy().await;
    let fwd = lease_forwarder(proxy).await;
    let (mut c, head) = send(
        fwd,
        b"CONNECT api.example.com:443 HTTP/1.1\r\nHost: x\r\n\
          Proxy-Authorization: Basic bGVhc2U6dG9r\r\n\r\n",
    )
    .await;
    assert_eq!(head, "HTTP/1.1 200 Connection established\r\n\r\n");
    c.write_all(b"ping").await.unwrap();
    assert_eq!(read_n(&mut c, 4).await, b"ping");
    let seen = seen.recv().await.unwrap();
    assert_eq!(seen.auth.as_deref(), Some("Basic bGVhc2U6dG9r"));
}

#[tokio::test]
async fn with_no_token_a_client_without_a_proxy_password_gets_407() {
    let (proxy, _seen) = echo_proxy().await;
    let fwd = lease_forwarder(proxy).await;
    let (_c, head) = send(
        fwd,
        b"CONNECT api.example.com:443 HTTP/1.1\r\nHost: x\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 407"), "{head}");
}

#[test]
fn a_config_with_no_token_passes_leases() {
    assert!(
        Config::parse(r#"{"listen":"127.0.0.1:1","upstream":"wss://moor.example/egress"}"#).is_ok()
    );
}

#[test]
fn the_config_names_its_listener_upstream_and_token() {
    let config = Config::parse(
        r#"{"listen":"127.0.0.1:20001","upstream":"wss://moor.example/egress","token":"t0k"}"#,
    )
    .unwrap();
    assert_eq!(config.listen, "127.0.0.1:20001".parse().unwrap());
    assert!(
        !format!("{:?}", config.upstream).contains("t0k"),
        "the token stays out of Debug"
    );
    for (bad, why) in [
        (
            r#"{"listen":"127.0.0.1:1","upstream":"https://x/egress","token":"t"}"#,
            "not a ws:// or wss://",
        ),
        (
            r#"{"listen":"127.0.0.1:1","upstream":"ws://127.0.0.1/egress","token":""}"#,
            "token is empty",
        ),
        (
            r#"{"listen":"127.0.0.1:1","upstream":"ws://[::1]:9/egress","token":"a\nb"}"#,
            "cannot be sent in a header",
        ),
        (r#"{"listen":"127.0.0.1:1"}"#, "bad config"),
        (
            r#"{"listen":"127.0.0.1:1","upstream":"ws://moor.example/egress","token":"t"}"#,
            "only a loopback ws:// address may",
        ),
        (
            r#"{"listen":"127.0.0.1:1","upstream":"ws://127.0.0.1.evil.example/egress","token":"t"}"#,
            "only a loopback ws:// address may",
        ),
    ] {
        let err = Config::parse(bad).err().unwrap();
        assert!(err.contains(why), "{bad}: {err}");
    }
}

#[test]
fn failed_accepts_back_off_from_10_ms_to_1_s() {
    let mut pause = Duration::from_millis(10);
    let mut seen = vec![pause];
    for _ in 0..8 {
        pause = next_accept_pause(pause);
        seen.push(pause);
    }
    assert_eq!(
        seen.iter().map(Duration::as_millis).collect::<Vec<_>>(),
        [10, 20, 40, 80, 160, 320, 640, 1000, 1000]
    );
}

/// A stand-in Proxy whose upstream answers a request only after a pause, and drops the
/// tunnel unanswered when a close frame comes first, as the shared Proxy does.
async fn slow_answering_proxy() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
        let Some(Ok(Message::Binary(req))) = ws.next().await else {
            return;
        };
        let next = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        if matches!(next, Ok(Some(Ok(Message::Close(_))) | None)) {
            return;
        }
        let mut reply = b"reply:".to_vec();
        reply.extend_from_slice(&req);
        ws.send(Message::binary(reply)).await.unwrap();
        ws.close(None).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn a_client_that_stops_sending_still_reads_the_whole_reply() {
    let proxy = slow_answering_proxy().await;
    let fwd = forwarder(format!("ws://{proxy}/egress"), RootCertStore::empty()).await;
    let (mut c, head) = send(fwd, b"CONNECT h:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    c.write_all(b"req").await.unwrap();
    c.shutdown().await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rest, b"reply:req");
}
