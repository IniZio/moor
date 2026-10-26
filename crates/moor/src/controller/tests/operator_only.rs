//! What only the operator may do, and what a front must assert, over the controller and front
//! sockets.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::{BoxId, BoxName};
use serde_json::{json, Value};

use super::authz::{
    acting, every_kind, naming, not_yours, response_line, Names, World, A, A_BOX, B, B_BOX,
    MISSING_SESSION,
};
use super::fronts::{ask, refused};

/// A request of `kind` for a kind that names no Box; `None` for a kind this test does not know.
fn nameless(kind: &str) -> Option<Value> {
    Some(match kind {
        "start-needs" | "env" | "repos" | "whoami" | "enroll" | "secret-names" | "link-code"
        | "version" | "proxy-lease" | "proxy-status" | "mcp-list" | "machine-list"
        | "machine-targets" => {
            json!({"cmd": kind})
        }
        "proxy-release" => json!({"cmd": kind, "token": "t"}),
        "session-binding" => json!({"cmd": kind, "front": "slack", "key": "k"}),
        "gate" => json!({"cmd": kind, "sha": "abcd123", "name": "test"}),
        "secret-set" => json!({"cmd": kind, "name": "n", "value": "v"}),
        "secret-delete" | "mcp-remove" | "machine-remove" => json!({"cmd": kind, "name": "n"}),
        "mcp-add" => json!({"cmd": kind, "name": "n", "url": "https://example.com/mcp"}),
        "machine-add" => json!({"cmd": kind, "name": "n", "ssh": "u@h"}),
        "link-redeem" => json!({"cmd": kind, "code": "c"}),
        "box-mirror" => json!({"cmd": kind, "id": 1, "dest": "/d"}),
        "box-put" => json!({"cmd": kind, "id": 1, "src": "/s", "dest": "d"}),
        "in" => json!({"cmd": kind, "dir": "/w", "request": {"cmd": "show"}}),
        _ => return None,
    })
}

#[test]
fn every_front_request_but_version_must_assert_an_identity() {
    let world = World::new();
    let slack = world.slack();
    let mut unknown = Vec::new();
    for kind in every_kind().iter().filter(|k| *k != "as") {
        let request = match naming(kind, B_BOX, world.b_session) {
            Names::Box(request) => request,
            Names::Nothing | Names::Unknown => {
                let Some(request) = nameless(kind) else {
                    unknown.push(kind.clone());
                    continue;
                };
                request
            }
        };
        let got = ask(&slack, &request);
        if kind == "version" {
            assert_eq!(got["class"], "ok", "{got}");
        } else {
            refused(&got, "the slack front socket must assert an identity");
        }
    }
    assert!(
        unknown.is_empty(),
        "say how to send these new request kinds: {unknown:?}"
    );
}

#[test]
fn machine_targets_lists_every_users_machines_to_the_operator_only() {
    let world = World::new();
    let (slack, main) = (world.slack(), &world.served.main);
    for (who, name, ssh) in [
        (A, "one", "a@one"),
        (B, "two", "b@two"),
        (B, "three", "b@three"),
    ] {
        let add = json!({"cmd": "machine-add", "name": name, "ssh": ssh});
        assert_eq!(ask(&slack, &acting(who, &add))["class"], "ok");
    }
    let listed = ask(main, &json!({"cmd": "machine-targets"}));
    assert_eq!(
        listed["value"]["targets"],
        json!(["a@one", "b@three", "b@two"]),
        "{listed}"
    );
    refused(
        &ask(&slack, &acting(A, &json!({"cmd": "machine-targets"}))),
        "machine-targets lists every user's machines, which is operator-only",
    );
}

#[test]
fn env_is_the_operators_only_and_repos_is_no_command() {
    let world = World::new();
    let slack = world.slack();
    refused(
        &ask(&slack, &acting(A, &json!({"cmd": "env", "repo": "o/n"}))),
        "env reads the host's configs, which is operator-only",
    );
    for socket in [&slack, &world.served.main] {
        let got = ask(socket, &json!({"cmd": "repos"}));
        assert_eq!(got["frame"], "error", "{got}");
    }
}

/// The ids of the Sessions a `sessions` reply lists.
fn session_ids(envelope: &Value) -> Vec<i64> {
    envelope["value"]["sessions"]
        .as_array()
        .expect("a list of Sessions")
        .iter()
        .map(|s| s["session"].as_i64().expect("id"))
        .collect()
}

#[test]
fn the_operators_host_sessions_are_the_operators_only() {
    let world = World::new();
    let (slack, main) = (world.slack(), &world.served.main);
    let opened = ask(
        main,
        &json!({"cmd": "open-host-session", "workdir": "/home/op/w"}),
    );
    let host = opened["value"]["session"].as_i64().expect("host Session");
    let bind = json!({"cmd": "bind-session", "front": "slack", "key": "thread",
        "session": host, "owner": "o"});
    assert_eq!(ask(main, &bind)["class"], "ok");
    for kind in [
        "show-session",
        "update-host-session",
        "end-host-session",
        "bind-session",
        "sessions",
        "open-host-session",
        "open-session",
        "start",
    ] {
        let (on_host, on_missing) = (on_session(kind, host), on_session(kind, MISSING_SESSION));
        not_yours(&ask(&slack, &acting(A, &on_host)));
        assert_eq!(
            response_line(&slack, &acting(A, &on_host)),
            response_line(&slack, &acting(A, &on_missing)),
            "{kind}"
        );
    }
    let listed = ask(&slack, &acting(A, &json!({"cmd": "sessions"})));
    assert!(!session_ids(&listed).contains(&host), "{listed}");
    let lookup = json!({"cmd": "session-binding", "front": "slack", "key": "thread"});
    assert_eq!(ask(&slack, &acting(A, &lookup))["value"], Value::Null);
    assert_eq!(ask(main, &lookup)["value"]["session"], host);
    assert!(session_ids(&ask(main, &json!({"cmd": "sessions"}))).contains(&host));
    let shown = ask(main, &json!({"cmd": "show-session", "session": host}));
    assert_eq!(shown["value"]["workdir"], "/home/op/w", "{shown}");
}

/// The kinds that act on a Session, each sent naming `session`.
const ON_SESSION: [&str; 8] = [
    "show-session",
    "update-host-session",
    "bind-session",
    "sessions",
    "open-host-session",
    "open-session",
    "start",
    "end-host-session",
];

#[test]
fn a_host_session_belongs_to_the_user_who_opened_it() {
    let world = World::new();
    let (slack, main) = (world.slack(), &world.served.main);
    let open = json!({"cmd": "open-host-session"});
    let opened = ask(&slack, &acting(A, &open));
    let host = opened["value"]["session"]
        .as_i64()
        .expect("A opens a host Session");
    for kind in ON_SESSION {
        not_yours(&ask(&slack, &acting(B, &on_session(kind, host))));
    }
    let listed = ask(&slack, &acting(B, &json!({"cmd": "sessions"})));
    assert!(!session_ids(&listed).contains(&host), "{listed}");
    let listed = ask(&slack, &acting(A, &json!({"cmd": "sessions"})));
    assert!(session_ids(&listed).contains(&host), "{listed}");
    assert!(session_ids(&ask(main, &json!({"cmd": "sessions"}))).contains(&host));
    for kind in ON_SESSION {
        let got = ask(&slack, &acting(A, &on_session(kind, host)));
        let message = got["error"]["message"].as_str().unwrap_or_default();
        assert_ne!(message, "not your Box", "{kind}: {got}");
    }
    let shown = ask(main, &json!({"cmd": "show-session", "session": host}));
    assert_eq!(
        shown["value"]["workdir"], opened["value"]["workdir"],
        "{shown}"
    );
}

#[test]
fn an_identity_with_no_user_cannot_open_a_host_session() {
    let world = World::new();
    let open = json!({"cmd": "open-host-session", "workdir": "/w"});
    refused(
        &ask(&world.slack(), &acting("slack:T1:NOBODY", &open)),
        "log in first: moor user enroll",
    );
}

/// A request of `kind` that names Session `session`, as its parent for the kinds that name a
/// Session only that way.
fn on_session(kind: &str, session: i64) -> Value {
    let named = match naming(kind, A_BOX, session) {
        Names::Box(request) => Some(request),
        Names::Nothing | Names::Unknown => None,
    };
    let mut request = named.expect("the kind names a Session");
    if matches!(kind, "start" | "open-session") {
        request["parent"] = json!(session);
    }
    request
}

#[test]
fn a_start_under_another_users_session_is_refused() {
    let world = World::new();
    let start = json!({"cmd": "start", "name": "new-box", "parent": world.b_session});
    not_yours(&ask(&world.slack(), &acting(A, &start)));
}

#[test]
fn a_session_opened_under_another_users_session_is_refused() {
    let world = World::new();
    let open =
        json!({"cmd": "open-session", "box": A_BOX, "seed": "go", "parent": world.b_session});
    not_yours(&ask(&world.slack(), &acting(A, &open)));
}

#[test]
fn show_session_lists_only_the_children_the_caller_may_see() {
    let world = World::new();
    let a_box = BoxId::new(1);
    let user_of = |id: BoxId| {
        let row = world.store.box_row(id).expect("read").expect("row");
        row.user.expect("a user")
    };
    let parent = world.store.box_sessions(a_box).expect("sessions")[0].id;
    for (name, owner) in [("a-child", a_box), ("b-child", world.b_box)] {
        let new = NewBox {
            name: BoxName::parse_allow_bare(name).expect("name"),
            repo: None,
        };
        let child = Decision::insert(new)
            .parent_session(Some(parent))
            .requester(user_of(owner));
        world.store.apply(child).expect("child Box");
    }
    let children = |envelope: Value| -> Vec<String> {
        let kids = envelope["value"]["children"].as_array().expect("children");
        kids.iter()
            .map(|c| c["box"].as_str().expect("box").to_owned())
            .collect()
    };
    let show = json!({"cmd": "show-session", "session": parent});
    assert_eq!(
        children(ask(&world.slack(), &acting(A, &show))),
        ["a-child"]
    );
    assert_eq!(
        children(ask(&world.served.main, &show)),
        ["a-child", "b-child"]
    );
}
