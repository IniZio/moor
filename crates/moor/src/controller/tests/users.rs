//! User requests end to end over the controller and front sockets.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::path::Path;

use sea_query::ExprTrait as _;
use serde_json::json;

use crate::store::db::DbExt as _;

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
    let db = crate::store::db::Conn::open_file(&state.join("moor.db")).expect("db");
    let events: i64 = db
        .row(
            sea_query::Query::select()
                .expr(sea_query::Expr::col(sea_query::Asterisk).count())
                .from(moor_schema::tables::Events::Table)
                .take(),
            |r| r.get(0),
        )
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
fn two_fronts_asserting_one_email_share_a_user_and_its_secrets() {
    let served = Served::start();
    let slack = served.front("slack");
    let web = served.front("web");
    let enroll = |socket: &Path, identity: &str, email: Option<&str>| {
        let mut req = json!({"cmd": "enroll", "identity": identity});
        if let Some(email) = email {
            req["email"] = json!(email);
        }
        ask(socket, &req)
    };
    let first = enroll(&slack, "slack:T1:U1", Some("alice@example.com"));
    assert_eq!(first["value"]["created"], true, "{first}");
    let set =
        json!({"cmd": "secret-set", "identity": "slack:T1:U1", "name": "claude", "value": CANARY});
    ask(&slack, &set);
    let second = enroll(&web, "web:sub-1", Some("Alice@Example.com"));
    assert_eq!(second["value"]["user"], first["value"]["user"], "{second}");
    assert_eq!(second["value"]["created"], false, "{second}");
    let names = ask(
        &web,
        &json!({"cmd": "secret-names", "identity": "web:sub-1"}),
    );
    assert_eq!(names["value"]["names"], json!(["claude"]), "{names}");
    let third = enroll(&web, "web:sub-2", None);
    assert_ne!(third["value"]["user"], first["value"]["user"], "{third}");
}

#[test]
fn only_a_front_socket_may_vouch_for_an_email() {
    let served = Served::start();
    let got = ask(
        &served.main,
        &json!({"cmd": "enroll", "email": "alice@example.com"}),
    );
    refused(&got, "only a front can vouch for an email");
    let machine = ask(
        &served.main,
        &json!({"cmd": "machine-enroll", "email": "alice@example.com", "name": "minion",
            "pubkey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"}),
    );
    refused(&machine, "only a front can vouch for an email");
    let bad = ask(
        &served.front("slack"),
        &json!({"cmd": "enroll", "identity": "slack:T1:U1", "email": "not-an-email"}),
    );
    assert_ne!(bad["class"], "ok", "{bad}");
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

#[test]
fn a_workflow_add_obeys_the_default_floor_and_takes_its_origin_from_the_socket() {
    let served = Served::start();
    let slack = served.front("slack");
    ask(&slack, &json!({"cmd": "enroll", "identity": "slack:U1"}));
    ask(&served.main, &json!({"cmd": "enroll"}));
    let add = |cron: &str, key: Option<&str>| {
        let mut req = json!({"cmd": "workflow-add", "identity": "slack:U1", "name": "daily",
            "cron": cron, "timezone": "UTC", "repos": ["o/n"], "prompt": "fix"});
        if let Some(key) = key {
            req["origin_key"] = json!(key);
        }
        req
    };
    let fast = ask(&slack, &add("* * * * *", None));
    assert_eq!(fast["error"]["kind"], "usage", "{fast}");
    let message = fast["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("* * * * *") && message.contains("60 minutes"),
        "{fast}"
    );

    let added = ask(&slack, &add("0 9 * * *", Some("C1/T1")));
    assert_eq!(added["class"], "ok", "{added}");
    let listed = ask(
        &slack,
        &json!({"cmd": "workflow-list", "identity": "slack:U1"}),
    );
    assert_eq!(listed["value"]["workflows"][0]["name"], "daily", "{listed}");

    let own = json!({"cmd": "workflow-add", "name": "mine", "cron": "0 9 * * *",
        "timezone": "UTC", "repos": ["o/n"], "prompt": "fix", "origin_key": "k"});
    refused(
        &ask(&served.main, &own),
        "only a front can send an origin_key",
    );
}
