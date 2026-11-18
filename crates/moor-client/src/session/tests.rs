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
fn sign_ins_to_two_servers_live_side_by_side() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    store.write(&creds("https://a.example")).unwrap();
    store.write(&creds("https://b.example")).unwrap();
    assert!(store.delete("https://a.example/").unwrap());
    let left: Vec<_> = store.all().unwrap().into_iter().map(|c| c.server).collect();
    assert_eq!(left, ["https://b.example"]);
}
