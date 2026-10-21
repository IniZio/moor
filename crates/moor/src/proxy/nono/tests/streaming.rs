//! An upload through a rule host must stream: the Proxy may hold only a
//! bounded window of the body, whatever the body's size.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use moor_core::{BoxId, Secret};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use tempfile::TempDir;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Proxy, SendBody};

use super::uploads::uploads_rule;
use super::upstream::make_certs;
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};

const TOKEN: &str = "token_AbCdEf-0123456789";
const MIB: u64 = 1 << 20;
const PATH: &str = "/user-attachments/assets?name=a.mp4&content_type=video%2Fmp4&repository_id=1";

/// Loopback HTTPS upstream for one request that counts body bytes as they arrive.
struct Sink {
    port: u16,
    ca_path: std::path::PathBuf,
    received: Arc<AtomicU64>,
}

fn sink(dir: &std::path::Path) -> Sink {
    let (ca_pem, leaf, key) = make_certs();
    let config = Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![leaf], key)
            .unwrap(),
    );
    let ca_path = dir.join("sink-ca.pem");
    std::fs::write(&ca_path, ca_pem).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&received);
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut tls = StreamOwned::new(ServerConnection::new(config).unwrap(), stream);
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if tls.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            head.push(byte[0]);
        }
        let len: u64 = String::from_utf8_lossy(&head)
            .lines()
            .find_map(|l| {
                let (n, v) = l.split_once(':')?;
                n.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse().ok())?
            })
            .unwrap_or(0);
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0;
        while got < len {
            match tls.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    got += n as u64;
                    counter.store(got, Ordering::SeqCst);
                }
            }
        }
        let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";
        let _ = tls.write_all(reply);
        let _ = tls.flush();
        tls.conn.send_close_notify();
        let _ = tls.flush();
    });
    Sink {
        port,
        ca_path,
        received,
    }
}

/// Body of `total` zero bytes that pauses after `gate` bytes until released.
struct Gated {
    total: u64,
    gate: u64,
    sent: Arc<AtomicU64>,
    release: mpsc::Receiver<()>,
    released: bool,
}

impl Read for Gated {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let sent = self.sent.load(Ordering::SeqCst);
        if sent >= self.gate && !self.released {
            let _ = self.release.recv_timeout(Duration::from_secs(30));
            self.released = true;
        }
        let limit = if self.released { self.total } else { self.gate };
        let n = usize::try_from(limit - sent).map_or(buf.len(), |left| left.min(buf.len()));
        buf.iter_mut().take(n).for_each(|b| *b = 0);
        self.sent.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

fn agent(proxy_port: u16, ca_pem: &str) -> Agent {
    let cert = Certificate::from_pem(ca_pem.as_bytes()).unwrap().to_owned();
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::Specific(Arc::new(vec![cert])))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let proxy = Proxy::new(&format!("http://moor:{TOKEN}@127.0.0.1:{proxy_port}")).unwrap();
    Agent::new_with_config(
        Agent::config_builder()
            .proxy(Some(proxy))
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .build(),
    )
}

/// Waits until the upstream count reaches `sent` or stops moving for 2 s
/// (10 s at most), then returns it.
fn settled(received: &AtomicU64, sent: u64) -> u64 {
    let (mut last, mut still_ticks) = (u64::MAX, 0);
    for _ in 0..200 {
        let now = received.load(Ordering::SeqCst);
        if now >= sent || still_ticks >= 40 {
            return now;
        }
        if now == last {
            still_ticks += 1;
        } else {
            (last, still_ticks) = (now, 0);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    received.load(Ordering::SeqCst)
}

/// Posts `total` bytes and pauses after `gate`. Returns the bytes held while
/// the client paused (sent by the client, not yet seen upstream), the final
/// status, and the bytes the upstream got.
fn upload(total: u64, gate: u64) -> (u64, Option<u16>, u64) {
    let dir = TempDir::new().unwrap();
    let up = sink(dir.path());
    let rules = vec![uploads_rule(format!("localhost:{}", up.port))];
    let source = RuleSource {
        config_key: "github".to_owned(),
        fetch: Arc::new(|| Ok(Secret::new(b"gho_real".to_vec()))),
        basic_user: None,
    };
    let capture = Arc::new(MoorCapture::new(BTreeMap::from([(
        "github-uploads".to_owned(),
        source,
    )])));
    let state = dir.path().join("state");
    let ca = ca::load_or_create(&state, BoxId::new(7)).unwrap();
    let token = Secret::new(TOKEN.as_bytes().to_vec());
    let mut cfg = build_config(&NonoSpec {
        rules: &rules,
        token: &token,
        ca: &ca,
        ca_dir: &ca::ca_dir(&state, BoxId::new(7)),
        upstream_ca: Some(&up.ca_path),
        chain: None,
    });
    cfg.0.upstream_extra_ca_pems = vec![up.ca_path.to_string_lossy().into_owned()];
    let proxy = NonoProxy::start(cfg, capture).unwrap();
    let agent = agent(proxy.port(), ca.cert_pem());
    let url = format!("https://localhost:{}{PATH}", up.port);
    let sent = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel();
    let mut body = Gated {
        total,
        gate,
        sent: Arc::clone(&sent),
        release: rx,
        released: false,
    };
    let client = std::thread::spawn(move || {
        agent
            .post(&url)
            .header("Authorization", "token gho_moor-proxy")
            .header("Content-Length", &total.to_string())
            .send(SendBody::from_reader(&mut body))
            .map(|r| r.status().as_u16())
            .ok()
    });
    for _ in 0..500 {
        if sent.load(Ordering::SeqCst) >= gate {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let at_gate = sent.load(Ordering::SeqCst);
    let held = at_gate - settled(&up.received, at_gate);
    let _ = tx.send(());
    let status = client.join().unwrap();
    let got = up.received.load(Ordering::SeqCst);
    proxy.stop();
    (held, status, got)
}

/// Held bytes must stay under this. Socket buffers on both legs count as
/// held, so the bound is well above the Proxy's 64 KiB relay window.
const WINDOW: u64 = 2 * MIB;

#[test]
fn a_9_mib_upload_reaches_upstream_while_the_client_is_still_sending() {
    let (held, status, got) = upload(9 * MIB, 8 * MIB);
    assert!(
        held < WINDOW,
        "Proxy held {held} bytes of the paused upload"
    );
    assert_eq!((status, got), (Some(200), 9 * MIB));
}

#[test]
fn a_50_mib_upload_streams_through_a_bounded_window() {
    let (held, status, got) = upload(50 * MIB, 40 * MIB);
    assert!(
        held < WINDOW,
        "Proxy held {held} bytes of the paused upload"
    );
    assert_eq!((status, got), (Some(200), 50 * MIB));
}
