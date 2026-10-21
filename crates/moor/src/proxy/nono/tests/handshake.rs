//! A TLS client that ends the handshake with the intercepting proxy, and the reason recorded.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use super::e2e::{rig, Rig};
use crate::proxy::nono::audit::{self, BoxSink, Withheld};

/// Opens a CONNECT through the proxy to the header upstream and hands the tunnel to `client`.
fn connect_then(rig: &Rig, client: impl FnOnce(TcpStream)) {
    let mut sock = TcpStream::connect(("127.0.0.1", rig.proxy().port())).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let target = format!("localhost:{}", rig.up.port);
    write!(
        sock,
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\
         Proxy-Authorization: Basic bW9vcjp0b2tlbl9BYkNkRWYtMDEyMzQ1Njc4OQ==\r\n\r\n"
    )
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        sock.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"), "{head:?}");
    client(sock);
}

/// The `reason` of the one `egress_refused` event a handshake that `client` ends records.
fn handshake_reason(client: impl FnOnce(TcpStream)) -> Option<String> {
    let rig = rig();
    connect_then(&rig, client);
    let (store, box_id, log) = rig.store();
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let mut refused = None;
    for _ in 0..100 {
        audit::apply(
            &rig.proxy().drain_audit(),
            &rig.capture,
            &Withheld::default(),
            &sink,
        );
        let events = store.events(box_id, 100).unwrap();
        refused = events.into_iter().find(|e| e.kind == "egress_refused");
        if refused.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let payload = refused.expect("no egress_refused event").payload;
    assert_eq!(
        payload.get("category").and_then(|c| c.as_str()),
        Some("intercept_handshake_failed")
    );
    payload
        .get("reason")
        .and_then(|r| r.as_str())
        .map(str::to_owned)
}

#[test]
fn client_hanging_up_mid_handshake_is_recorded_as_client_closed() {
    let reason = handshake_reason(|mut sock| {
        sock.write_all(&[0x16, 0x03, 0x01, 0x00, 0x20, 0x01])
            .unwrap();
        drop(sock);
    });
    assert_eq!(reason.as_deref(), Some("client_closed"));
}

#[test]
fn client_rejecting_the_box_ca_is_recorded_as_an_unknown_ca_alert() {
    let reason = handshake_reason(|mut sock| {
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
        let mut conn =
            rustls::ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap())
                .unwrap();
        assert!(conn.complete_io(&mut sock).is_err());
        while conn.wants_write() {
            conn.write_tls(&mut sock).unwrap();
        }
    });
    assert_eq!(reason.as_deref(), Some("unknown_ca_alert"));
}
