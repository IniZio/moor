//! nono chains an intercepted, credential-injected route through an external
//! proxy on loopback, and a filter there that terminates TLS with the Box CA
//! sees the whole request, body included.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::{BoxId, Secret};
use nono_proxy::config::{EndpointRule, ExternalProxyConfig, InjectMode, RouteConfig};
use nono_proxy::tls_intercept::{build_server_config, CertCache, EphemeralCa};
use nono_proxy::ProxyConfig;
use rustls::{ServerConnection, StreamOwned};
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::upstream;
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{ca, route_prefix};
use crate::testing::clock::FakeClock;

/// The credential the route injects.
const SECRET: &str = "fake-chain-credential-5d1e";
/// The session token nono checks, sent as the proxy password.
const TOKEN: &str = "token_AbCdEf-0123456789";
/// Box id whose CA directory the proxy uses.
const BOX_ID: BoxId = BoxId::new(9);
/// The GraphQL body the client posts.
const BODY: &str =
    r#"{"query":"mutation { addStar(input:{starrableId:\"X\"}) { clientMutationId } }"}"#;
/// What the filter answers.
const REPLY: &str = r#"{"data":{"ok":true}}"#;

/// What the fake filter observed.
#[derive(Debug, Default, Clone)]
struct Captured {
    /// The CONNECT request head nono sent.
    connect: String,
    /// The tunnelled request head.
    head: String,
    /// The tunnelled request body.
    body: Vec<u8>,
}

/// A fake filter: CONNECT, then TLS under the Box CA, then one request.
struct Filter {
    /// Loopback port it listens on.
    port: u16,
    /// What it saw.
    seen: Arc<Mutex<Captured>>,
    /// Serving thread.
    thread: Option<JoinHandle<()>>,
}

impl Filter {
    /// What the filter saw once its thread finished.
    fn finish(mut self) -> Captured {
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Reads up to and including the blank line that ends a head.
fn read_head(r: &mut impl Read) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if r.read(&mut byte).unwrap_or(0) == 0 || buf.len() > 16 * 1024 {
            break;
        }
        buf.push(byte[0]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Value of header `name` in a head.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// Serves one chained connection.
fn serve(mut tcp: TcpStream, ca: &ca::BoxCa, seen: &Mutex<Captured>) {
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let connect = read_head(&mut tcp);
    seen.lock().unwrap_or_else(PoisonError::into_inner).connect = connect;
    tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .unwrap();
    let key = ca.preloaded().key_der;
    let box_ca = Arc::new(EphemeralCa::from_existing(&key, ca.cert_pem()).unwrap());
    let config = build_server_config(Arc::new(CertCache::new(box_ca)), false).unwrap();
    let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), tcp);
    let head = read_head(&mut tls);
    let len: usize = header(&head, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    tls.read_exact(&mut body).unwrap();
    {
        let mut guard = seen.lock().unwrap_or_else(PoisonError::into_inner);
        guard.head = head;
        guard.body = body;
    }
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{REPLY}",
        REPLY.len()
    );
    tls.write_all(reply.as_bytes()).unwrap();
    tls.flush().unwrap();
    tls.conn.send_close_notify();
    let _ = tls.flush();
}

/// Starts the filter; it serves the first connection it gets.
fn start_filter(ca: ca::BoxCa) -> Filter {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Captured::default()));
    let shared = Arc::clone(&seen);
    let thread = std::thread::spawn(move || {
        for _ in 0..750 {
            if let Ok((tcp, _)) = listener.accept() {
                tcp.set_nonblocking(false).unwrap();
                serve(tcp, &ca, &shared);
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    Filter {
        port,
        seen,
        thread: Some(thread),
    }
}

/// nono config: one `api.github.com` route chained through `filter_port`.
fn chained_config(ca: &ca::BoxCa, ca_dir: &Path, filter_port: u16) -> ProxyConfig {
    let route = RouteConfig {
        prefix: route_prefix("gh", 0),
        upstream: "https://api.github.com".to_owned(),
        credential_key: Some("cmd://gh".to_owned()),
        inject_mode: InjectMode::Header,
        inject_header: "authorization".to_owned(),
        credential_format: Some("token {}".to_owned()),
        endpoint_rules: vec![EndpointRule {
            method: "POST".to_owned(),
            path: "/graphql".to_owned(),
        }],
        tls_ca: Some(ca_dir.join("ca.pem").to_string_lossy().into_owned()),
        ..RouteConfig::default()
    };
    ProxyConfig {
        bind_addr: Ipv4Addr::LOCALHOST.into(),
        bind_port: 0,
        allowed_hosts: vec!["api.github.com".to_owned()],
        strict_filter: true,
        require_auth: true,
        strict_connect_auth: true,
        session_token: Some(Zeroizing::new(TOKEN.to_owned())),
        routes: vec![route],
        intercept_ca_dir: Some(ca_dir.to_path_buf()),
        preloaded_ca: Some(ca.preloaded()),
        enable_h2: false,
        enable_network_audit: true,
        external_proxy: Some(ExternalProxyConfig {
            address: format!("127.0.0.1:{filter_port}"),
            auth: None,
            bypass_hosts: Vec::new(),
        }),
        ..ProxyConfig::default()
    }
}

/// Capture backend that hands `SECRET` to rule `gh`.
fn capture() -> Arc<MoorCapture> {
    let source = RuleSource {
        config_key: "credentials.TOKEN".to_owned(),
        fetch: Arc::new(|| Ok(Secret::new(SECRET.as_bytes().to_vec()))),
        basic_user: None,
    };
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    Arc::new(MoorCapture::new(
        BTreeMap::from([("gh".to_owned(), source)]),
        clock,
    ))
}

#[test]
fn intercepted_route_chains_through_external_filter_with_body_visible() {
    let dir = TempDir::new().unwrap();
    let state = dir.path().join("state");
    let ca = ca::load_or_create(&state, BOX_ID).unwrap();
    let ca_dir = ca::ca_dir(&state, BOX_ID);
    let filter = start_filter(ca::load_or_create(&state, BOX_ID).unwrap());
    let cfg = chained_config(&ca, &ca_dir, filter.port);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let handle = runtime
        .block_on(
            nono_proxy::server::start_with_approval_and_capture_registry(
                cfg,
                None,
                Some(capture()),
            ),
        )
        .unwrap();

    let client = upstream::client(handle.port, TOKEN, ca.cert_pem());
    let mut response = client
        .post("https://api.github.com/graphql")
        .header("Authorization", "Bearer placeholder-not-real")
        .header("Content-Type", "application/json")
        .send(BODY)
        .unwrap();
    let status = response.status().as_u16();
    let reply = response.body_mut().read_to_string().unwrap();

    let seen = filter.finish();
    let audit = handle.drain_audit_events();
    handle.shutdown();
    runtime.shutdown_timeout(Duration::from_millis(500));

    assert!(
        seen.connect
            .starts_with("CONNECT api.github.com:443 HTTP/1.1"),
        "filter saw: {}",
        seen.connect
    );
    assert!(
        header(&seen.head, "proxy-authorization").is_none()
            && !seen.connect.to_lowercase().contains("proxy-authorization"),
        "nono forwarded proxy credentials to the filter"
    );
    assert!(seen.head.starts_with("POST /graphql HTTP/1.1"));
    assert_eq!(
        header(&seen.head, "authorization"),
        Some(format!("token {SECRET}").as_str())
    );
    assert_eq!(seen.body, BODY.as_bytes());
    assert_eq!((status, reply.as_str()), (200, REPLY));
    assert!(!audit.is_empty(), "network audit recorded nothing");
}
