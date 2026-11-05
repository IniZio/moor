//! Nothing the shared Proxy logs carries a credential or the vault key.

use std::time::Duration;

use moor_core::user::Identity;

use super::{open, World, SECRET};

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
