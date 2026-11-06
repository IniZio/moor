//! Nothing the shared Proxy logs carries a credential or the vault key.

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::Duration;

use moor_core::user::Identity;

use super::{addr, open, World, PATH, SECRET, TARGET};

/// Every `.log` file under `dir`, concatenated.
fn logs_under(dir: &std::path::Path) -> Vec<u8> {
    let mut said = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()) {
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "log") {
                said.extend(std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    said
}

#[test]
fn no_secret_reaches_a_log_of_the_shared_proxy() {
    let w = World::new();
    let server = w.server(10.0);
    let tunnel = open(&server, &w.token, &w.target()).unwrap();
    assert_eq!(w.get(tunnel), "HTTP/1.1 200 OK");
    let (user, _) = w.store.enroll(&Identity::local(1), None).unwrap();
    let name = moor_core::user::SecretName::parse("tool-token").unwrap();
    assert!(w.store.delete_user_secret(user, &name).unwrap());
    if let Ok(tunnel) = open(&server, &w.token, &w.target()) {
        assert_ne!(
            w.get(tunnel),
            "HTTP/1.1 200 OK",
            "a deleted secret still worked"
        );
    }
    w.close_box();
    server.drain(Duration::from_secs(5));
    let mut said = w.log.lock().unwrap().clone();
    said.extend(logs_under(w.dir.path()));
    assert!(!said.is_empty(), "the Proxy logged something to scan");
    let key = std::fs::read_to_string(w.dir.path().join("vault.key")).unwrap();
    for secret in [SECRET, key.trim()] {
        let found = said.windows(secret.len()).any(|s| s == secret.as_bytes());
        assert!(!found, "a secret reached the Proxy's log");
    }
}

#[test]
fn a_503_names_no_detail_and_the_log_does() {
    let mut w = World::new();
    let server = w.server(10.0);
    drop(w.ledger.take());
    let mut raw = TcpStream::connect(addr(&server)).unwrap();
    write!(
        raw,
        "GET {PATH} HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Authorization: Bearer {}\r\n{TARGET}: {}\r\n\r\n",
        w.token,
        w.target()
    )
    .unwrap();
    let mut reply = String::new();
    raw.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 503"), "{reply}");
    assert!(
        reply.ends_with("\r\n\r\ntemporarily unavailable"),
        "{reply}"
    );
    let log = String::from_utf8(w.log.lock().unwrap().clone()).unwrap();
    assert!(log.contains("the controller is unavailable"), "{log}");
    server.drain(Duration::from_secs(5));
}
