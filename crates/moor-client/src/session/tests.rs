#![allow(clippy::unwrap_used, reason = "tests fail loudly")]

use super::{Credentials, Store};

fn creds(server: &str) -> Credentials {
    Credentials {
        server: server.to_owned(),
        issuer: "https://issuer.example".to_owned(),
        client_id: "c".to_owned(),
        token_endpoint: "https://issuer.example/token".to_owned(),
        refresh_token: "refresh-0".to_owned(),
        access_token: "access-0".to_owned(),
        expires_at: 0,
        email: None,
    }
}

#[test]
fn a_herdr_sign_in_is_imported_once_and_stays_gone_after_logout() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("herdr/credentials.json");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, serde_json::to_vec(&creds("https://moor.example/")).unwrap()).unwrap();
    let store = Store {
        dir: dir.path().join("moor"),
        legacy: Some(old),
    };
    let got = store.read("https://moor.example/mcp").unwrap().unwrap();
    assert_eq!(got.refresh_token, "refresh-0");
    assert!(store.read("https://other.example").unwrap().is_none());
    assert!(store.delete("https://moor.example").unwrap());
    assert!(store.read("https://moor.example").unwrap().is_none());
}

#[test]
fn sign_ins_to_two_servers_live_side_by_side() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.write(&creds("https://a.example")).unwrap();
    store.write(&creds("https://b.example")).unwrap();
    assert!(store.delete("https://a.example/").unwrap());
    let left: Vec<_> = store.all().unwrap().into_iter().map(|c| c.server).collect();
    assert_eq!(left, ["https://b.example"]);
}
