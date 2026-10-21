//! A TLS client that ends the handshake with the intercepting proxy, and the event recorded.

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

/// The kind and payload of the first handshake event a handshake that `client` ends records.
fn handshake_event(client: impl FnOnce(TcpStream)) -> (String, serde_json::Value) {
    let rig = rig();
    connect_then(&rig, client);
    let (store, box_id, log) = rig.store();
    let sink = BoxSink {
        log: &log,
        store: &store,
        box_id,
    };
    let mut found = None;
    for _ in 0..100 {
        audit::apply(&rig.proxy().drain_audit(), &Withheld::default(), &sink);
        let events = store.events(box_id, 100).unwrap();
        found = events
            .into_iter()
            .find(|e| e.kind == "egress_refused" || e.kind == "egress_aborted");
        if found.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let event = found.expect("no handshake event");
    (event.kind, event.payload)
}

#[test]
fn client_hanging_up_mid_handshake_is_recorded_as_aborted() {
    let (kind, payload) = handshake_event(|mut sock| {
        sock.write_all(&[0x16, 0x03, 0x01, 0x00, 0x20, 0x01])
            .unwrap();
        drop(sock);
    });
    assert_eq!(kind, "egress_aborted", "{payload}");
    assert!(payload.get("reason").is_none(), "{payload}");
}

#[test]
fn client_rejecting_the_box_ca_is_refused_as_an_unknown_ca_alert() {
    let (kind, payload) = handshake_event(|mut sock| {
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
    assert_eq!(kind, "egress_refused", "{payload}");
    let field = |name: &str| payload.get(name).and_then(|v| v.as_str());
    assert_eq!(field("category"), Some("intercept_handshake_failed"));
    assert_eq!(field("reason"), Some("unknown_ca_alert"));
}
