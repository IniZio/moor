//! User requests end to end over the controller and front sockets.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use serde_json::json;

use super::fronts::{ask, me, refused, Served};

const CANARY: &str = "sk-ant-oat01-canary-5be1c0";

#[test]
fn a_secret_value_is_in_no_reply_log_event_or_plaintext_db_byte() {
    let served = Served::start();
    ask(&served.main, &json!({"cmd": "enroll"}));
    let set = ask(
        &served.main,
        &json!({"cmd": "secret-set", "name": "claude", "value": CANARY}),
    );
    assert_eq!(set["class"], "ok", "{set}");
    let names = ask(&served.main, &json!({"cmd": "secret-names"}));
    assert_eq!(names["value"]["names"], json!(["claude"]), "{names}");
    for reply in [&set, &names] {
        assert!(!reply.to_string().contains(CANARY), "{reply}");
    }
    assert!(!served.log.text().contains(CANARY), "{}", served.log.text());
    let state = served.dir.path().join("state");
    let db = rusqlite::Connection::open(state.join("moor.db")).expect("db");
    let events: i64 = db
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("events");
    assert_eq!(events, 0, "user requests record no events");
    drop(db);
    for file in ["moor.db", "moor.db-wal", "moor.db-shm"] {
        let bytes = std::fs::read(state.join(file)).unwrap_or_default();
        let found = bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes());
        assert!(!found, "{file} holds the plaintext");
    }
}

#[test]
fn a_code_minted_on_the_controller_socket_links_a_slack_identity() {
    let served = Served::start();
    let user = ask(&served.main, &json!({"cmd": "enroll"}))["value"]["user"].clone();
    let minted = ask(&served.main, &json!({"cmd": "link-code"}));
    let code = minted["value"]["code"].clone();
    let slack = served.front("slack");
    let redeem = json!({"cmd": "link-redeem", "identity": "slack:T1:U1", "code": code});
    let linked = ask(&slack, &redeem);
    assert_eq!(linked["value"]["user"], user, "{linked}");
    let who = ask(&slack, &json!({"cmd": "whoami", "identity": "slack:T1:U1"}));
    assert_eq!(
        who["value"]["identities"],
        json!([me(), "slack:T1:U1"]),
        "{who}"
    );
    let reused = json!({"cmd": "link-redeem", "identity": "slack:T1:U2", "code": code});
    refused(&ask(&slack, &reused), "unknown or used link code");
}

#[test]
fn a_front_socket_cannot_act_for_the_local_user() {
    let served = Served::start();
    ask(&served.main, &json!({"cmd": "enroll"}));
    let slack = served.front("slack");
    for request in [
        json!({"cmd": "enroll", "identity": me()}),
        json!({"cmd": "secret-set", "identity": me(), "name": "claude", "value": CANARY}),
        json!({"cmd": "secret-names", "identity": me()}),
        json!({"cmd": "link-code", "identity": me()}),
    ] {
        let got = ask(&slack, &request);
        refused(&got, "may assert only slack:");
        assert!(!got.to_string().contains(CANARY), "{got}");
    }
    let names = ask(&served.main, &json!({"cmd": "secret-names"}));
    assert_eq!(names["value"]["names"], json!([]), "{names}");
}

#[test]
fn the_controller_socket_cannot_act_for_a_slack_user() {
    let served = Served::start();
    let slack = served.front("slack");
    ask(&slack, &json!({"cmd": "enroll", "identity": "slack:T1:U1"}));
    let set =
        json!({"cmd": "secret-set", "identity": "slack:T1:U1", "name": "claude", "value": "v"});
    refused(&ask(&served.main, &set), "speaks only for local:");
    let names = json!({"cmd": "secret-names", "identity": "slack:T1:U1"});
    assert_eq!(ask(&slack, &names)["value"]["names"], json!([]));
}
