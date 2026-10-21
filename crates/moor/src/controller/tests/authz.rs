//! Only a Box's own user and the local operator act on it, over the controller and front
//! sockets.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;

use moor_core::controller::{WatchFilter, PROTOCOL};
use moor_core::domain::decision::{Decision, ExtraEvent, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::user::{UserId, Viewer};
use moor_core::{BoxId, BoxName, Request};
use serde_json::{json, Value};

use super::fronts::{ask, me, refused, Served};
use crate::controller::watch::{stream, Ask, Limits};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const A: &str = "slack:T1:A";
const B: &str = "slack:T1:B";
const OPERATOR_ON_SLACK: &str = "slack:T1:OP";
const A_BOX: &str = "a-box";
const B_BOX: &str = "b-box";
const NO_USER_BOX: &str = "fixture-box";
const OPERATOR_BOX: &str = "op-box";
const MISSING: &str = "missing-box";
const MISSING_SESSION: i64 = 9999;

/// A controller with Boxes for users A and B, the operator's user and no user.
struct World {
    served: Served,
    store: Store,
    b_box: BoxId,
    b_session: i64,
    no_user_box: BoxId,
}

impl World {
    fn new() -> Self {
        let served = Served::start();
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let db = served.dir.path().join("state").join("moor.db");
        let store = Store::open(&db, clock).expect("store");
        let slack = served.front("slack");
        let enroll = |socket: &Path, identity: Option<&str>| {
            let mut req = json!({"cmd": "enroll"});
            if let Some(id) = identity {
                req["identity"] = json!(id);
            }
            let user = ask(socket, &req)["value"]["user"].as_i64().expect("user");
            UserId::new(user)
        };
        let (a, b) = (enroll(&slack, Some(A)), enroll(&slack, Some(B)));
        let operator = enroll(&served.main, None);
        let code = ask(&served.main, &json!({"cmd": "link-code"}))["value"]["code"].clone();
        let redeem = json!({"cmd": "link-redeem", "identity": OPERATOR_ON_SLACK, "code": code});
        assert_eq!(ask(&slack, &redeem)["class"], "ok");
        insert(&store, A_BOX, Some(a));
        insert(&store, OPERATOR_BOX, Some(operator));
        let b_box = insert(&store, B_BOX, Some(b));
        let no_user_box = insert(&store, NO_USER_BOX, None);
        let b_session = store.box_sessions(b_box).expect("sessions")[0].id;
        Self {
            served,
            store,
            b_box,
            b_session,
            no_user_box,
        }
    }

    /// The `slack` front's socket.
    fn slack(&self) -> std::path::PathBuf {
        self.served.front("slack")
    }
}

/// Inserts open Box `name`, acting for `user`, with one event.
fn insert(store: &Store, name: &str, user: Option<UserId>) -> BoxId {
    let new = Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(name).expect("name"),
        repo: "r".into(),
    });
    let new = match user {
        Some(user) => new.requester(user),
        None => new,
    };
    let id = store.apply(new).expect("box").box_id;
    store
        .apply(Decision::new(id).event(ExtraEvent::Attached))
        .expect("event");
    id
}

/// `request`, run as `identity`.
fn acting(identity: &str, request: &Value) -> Value {
    json!({"cmd": "as", "identity": identity, "request": request})
}

/// The response line `socket` sends for `request`, byte for byte.
fn response_line(socket: &Path, request: &Value) -> String {
    let mut write = UnixStream::connect(socket).expect("connect");
    let hello = json!({"frame": "hello", "protocol": PROTOCOL});
    let frame = json!({"frame": "request", "id": "1", "request": request});
    writeln!(write, "{hello}\n{frame}").expect("send");
    let mut read = BufReader::new(write);
    let mut line = String::new();
    loop {
        line.clear();
        read.read_line(&mut line).expect("frame");
        let got: Value = serde_json::from_str(&line).expect("json frame");
        if got["frame"] == "response" {
            return line;
        }
    }
}

/// Asserts `envelope` is the one refusal for a Box the caller may not act on.
fn not_yours(envelope: &Value) {
    refused(envelope, "not your Box");
    assert_eq!(envelope["error"]["message"], "not your Box", "{envelope}");
}

/// Every request kind, read from the variants serde accepts, so a new kind joins this list
/// without anyone touching it.
fn every_kind() -> Vec<String> {
    let error = serde_json::from_value::<Request>(json!({"cmd": "?"}))
        .expect_err("no such kind")
        .to_string();
    let (_, kinds) = error
        .split_once("expected one of")
        .expect("serde lists the kinds");
    kinds
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// What a request kind names.
enum Names {
    /// A request of the kind that names the given Box or Session.
    Box(Value),
    /// The kind names no Box.
    Nothing,
    /// A kind this test does not know yet.
    Unknown,
}

/// A request of `kind` that names Box `name` or Session `session`.
fn naming(kind: &str, name: &str, session: i64) -> Names {
    let on_box = |extra: Value| {
        let mut req = json!({"cmd": kind, "box": name});
        if let (Some(req), Value::Object(extra)) = (req.as_object_mut(), extra) {
            req.extend(extra);
        }
        req
    };
    Names::Box(match kind {
        "start" => json!({"cmd": kind, "name": name}),
        "run" => on_box(json!({"argv": ["true"]})),
        "wait" => on_box(json!({"timeout_secs": 0})),
        "wait-events" => json!({"cmd": kind, "boxes": [name], "timeout_secs": 0}),
        "steer" => on_box(json!({"text": "hi"})),
        "open-session" => on_box(json!({"seed": "go"})),
        "cp" => on_box(json!({"src": "a", "dest": "b"})),
        "artifact-read" => on_box(json!({"name": "n", "offset": 0, "len": 1})),
        "end-session" | "show" | "close" | "open" | "sleep" | "wake" | "connect" | "artifacts" => {
            on_box(json!({}))
        }
        "show-session" | "update-host-session" | "end-host-session" => {
            json!({"cmd": kind, "session": session})
        }
        "sessions" => json!({"cmd": kind, "parent": session}),
        "open-host-session" => json!({"cmd": kind, "workdir": "/w", "parent": session}),
        "bind-session" => {
            json!({"cmd": kind, "front": "slack", "key": "k", "session": session, "owner": "o"})
        }
        "start-needs" | "session-binding" | "gate" | "env" | "repos" | "whoami" | "enroll"
        | "secret-set" | "secret-delete" | "secret-names" | "link-code" | "link-redeem" | "as"
        | "in" | "version" => return Names::Nothing,
        _ => return Names::Unknown,
    })
}

#[test]
fn every_request_kind_that_names_a_box_is_refused_for_another_user() {
    let world = World::new();
    let slack = world.slack();
    let kinds = every_kind();
    assert!(kinds.len() > 30, "{kinds:?}");
    let (mut checked, mut unknown) = (0, Vec::new());
    for kind in &kinds {
        match naming(kind, B_BOX, world.b_session) {
            Names::Box(foreign) => {
                not_yours(&ask(&slack, &acting(A, &foreign)));
                checked += 1;
            }
            Names::Nothing => {}
            Names::Unknown => unknown.push(kind),
        }
    }
    assert!(
        unknown.is_empty(),
        "say whether these new request kinds name a Box: {unknown:?}"
    );
    assert_eq!(checked, 22, "{kinds:?}");
}

#[test]
fn a_missing_box_and_another_users_box_get_byte_identical_replies() {
    let world = World::new();
    let slack = world.slack();
    for kind in every_kind().iter().filter(|k| *k != "start") {
        let (Names::Box(foreign), Names::Box(missing)) = (
            naming(kind, B_BOX, world.b_session),
            naming(kind, MISSING, MISSING_SESSION),
        ) else {
            continue;
        };
        assert_eq!(
            response_line(&slack, &acting(A, &foreign)),
            response_line(&slack, &acting(A, &missing)),
            "{kind}"
        );
    }
    let by_id = |id: i64| json!({"cmd": "steer", "box": {"id": id}, "text": "hi"});
    assert_eq!(
        response_line(&slack, &acting(A, &by_id(world.b_box.get()))),
        response_line(&slack, &acting(A, &by_id(4242))),
    );
}

#[test]
fn a_user_runs_steers_closes_and_shows_only_its_own_box() {
    let world = World::new();
    let slack = world.slack();
    let show = |name: &str| json!({"cmd": "show", "box": name});
    let got = ask(&slack, &acting(A, &show(A_BOX)));
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(got["value"]["box"]["name"], A_BOX, "{got}");
    for request in [
        json!({"cmd": "run", "box": B_BOX, "argv": ["true"]}),
        json!({"cmd": "steer", "box": B_BOX, "text": "hi"}),
        json!({"cmd": "close", "box": B_BOX, "force": true}),
        show(B_BOX),
    ] {
        not_yours(&ask(&slack, &acting(A, &request)));
    }
    let b = world
        .store
        .box_row(world.b_box)
        .expect("read")
        .expect("row");
    assert!(b.lifecycle.is_live(), "B's Box stays open");
}

#[test]
fn the_operator_acts_on_every_box() {
    let world = World::new();
    for name in [A_BOX, B_BOX, NO_USER_BOX] {
        let got = ask(&world.served.main, &json!({"cmd": "show", "box": name}));
        assert_eq!(got["class"], "ok", "{got}");
        assert_eq!(got["value"]["box"]["name"], name, "{got}");
    }
    for request in [
        json!({"cmd": "steer", "box": B_BOX, "text": "hi"}),
        json!({"cmd": "close", "box": NO_USER_BOX, "force": true}),
    ] {
        let got = ask(&world.served.main, &request);
        assert_ne!(got["error"]["message"], "not your Box", "{got}");
    }
    let closed = world
        .store
        .box_row(world.no_user_box)
        .expect("read")
        .expect("row");
    assert!(!closed.lifecycle.is_live(), "the operator closed it");
}

#[test]
fn a_front_socket_is_never_the_operator_whatever_it_asserts() {
    let world = World::new();
    let slack = world.slack();
    let show = |name: &str| json!({"cmd": "show", "box": name});
    refused(
        &ask(&slack, &acting(&me(), &show(B_BOX))),
        "may assert only slack:",
    );
    refused(&ask(&slack, &show(B_BOX)), "must assert an identity");
    let got = ask(&slack, &acting(OPERATOR_ON_SLACK, &show(OPERATOR_BOX)));
    assert_eq!(
        got["class"], "ok",
        "the operator's user owns its Box: {got}"
    );
    for name in [B_BOX, NO_USER_BOX] {
        not_yours(&ask(&slack, &acting(OPERATOR_ON_SLACK, &show(name))));
    }
}

#[test]
fn a_box_with_no_user_is_the_operators_only() {
    let world = World::new();
    let slack = world.slack();
    let show = json!({"cmd": "show", "box": NO_USER_BOX});
    for user in [A, B] {
        not_yours(&ask(&slack, &acting(user, &show)));
    }
    let got = ask(&world.served.main, &show);
    assert_eq!(got["class"], "ok", "{got}");
}

/// The names of the Boxes a `show` with no Box lists.
fn listed(envelope: &Value) -> Vec<String> {
    let boxes = envelope["value"]["boxes"]
        .as_array()
        .expect("a list of Boxes");
    let mut names: Vec<String> = boxes
        .iter()
        .map(|b| b["name"].as_str().expect("name").to_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_list_shows_only_the_callers_boxes_and_the_operator_sees_all() {
    let world = World::new();
    let slack = world.slack();
    let list = json!({"cmd": "show"});
    assert_eq!(listed(&ask(&slack, &acting(A, &list))), [A_BOX]);
    assert_eq!(listed(&ask(&slack, &acting(B, &list))), [B_BOX]);
    assert_eq!(listed(&ask(&slack, &list)), Vec::<String>::new());
    assert_eq!(
        listed(&ask(&world.served.main, &list)),
        [A_BOX, B_BOX, NO_USER_BOX, OPERATOR_BOX]
    );
    let sessions = ask(&slack, &acting(A, &json!({"cmd": "sessions"})));
    let boxes: Vec<&Value> = sessions["value"]["sessions"]
        .as_array()
        .expect("a list of Sessions")
        .iter()
        .map(|s| &s["box"])
        .collect();
    assert_eq!(boxes, [&json!(A_BOX)], "{sessions}");
}

/// The Box names of the events a watch as `viewer` delivers.
fn watched(world: &World, viewer: Viewer) -> Vec<String> {
    let env = super::env(world.served.dir.path());
    let ask = Ask {
        since: 0,
        filter: WatchFilter::default(),
        follow: false,
        viewer,
    };
    let mut out = Vec::new();
    stream(&mut out, &env, ask, Limits::default());
    let mut names: Vec<String> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("frame"))
        .filter(|f| f["frame"] == "event")
        .map(|f| f["box"].as_str().expect("box").to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The Box names of the events a watch over `socket` delivers until it catches up.
fn watched_over(socket: &Path) -> Vec<String> {
    let mut write = UnixStream::connect(socket).expect("connect");
    let hello = json!({"frame": "hello", "protocol": PROTOCOL});
    let watch = json!({"frame": "watch", "since": 0, "follow": false});
    writeln!(write, "{hello}\n{watch}").expect("send");
    let mut names = Vec::new();
    for line in BufReader::new(write).lines() {
        let frame: Value = serde_json::from_str(&line.expect("line")).expect("frame");
        match frame["frame"].as_str() {
            Some("event") => names.push(frame["box"].as_str().expect("box").to_owned()),
            Some("caught-up") => break,
            _ => {}
        }
    }
    names.sort();
    names.dedup();
    names
}

#[test]
fn a_watch_on_a_front_socket_carries_no_events_and_the_operator_sees_all() {
    let world = World::new();
    assert_eq!(watched_over(&world.slack()), Vec::<String>::new());
    assert_eq!(
        watched_over(&world.served.main),
        [A_BOX, B_BOX, NO_USER_BOX, OPERATOR_BOX]
    );
}

#[test]
fn a_watch_carries_only_the_events_of_boxes_its_viewer_may_see() {
    let world = World::new();
    let a = world
        .store
        .box_row(BoxId::new(1))
        .expect("read")
        .expect("row");
    let a_user = a.user.expect("A's Box has a user");
    assert_eq!(watched(&world, Viewer::User(a_user)), [A_BOX]);
    assert_eq!(watched(&world, Viewer::Nobody), Vec::<String>::new());
    assert_eq!(
        watched(&world, Viewer::Operator),
        [A_BOX, B_BOX, NO_USER_BOX, OPERATOR_BOX]
    );
}

#[test]
fn a_binding_to_another_users_box_reads_as_no_binding() {
    let world = World::new();
    let slack = world.slack();
    let bind = json!({"cmd": "bind-session", "front": "slack", "key": "k1",
        "session": world.b_session, "owner": "o"});
    assert_eq!(ask(&slack, &acting(B, &bind))["class"], "ok");
    let lookup = |key: &str| json!({"cmd": "session-binding", "front": "slack", "key": key});
    let foreign = ask(&slack, &acting(A, &lookup("k1")));
    assert_eq!(foreign, ask(&slack, &acting(A, &lookup("k2"))));
    assert_eq!(foreign["value"], Value::Null, "{foreign}");
    let mine = ask(&slack, &acting(B, &lookup("k1")));
    assert_eq!(mine["value"]["session"], world.b_session, "{mine}");
    let a_session = world.store.box_sessions(BoxId::new(1)).expect("sessions")[0].id;
    let squat = json!({"cmd": "bind-session", "front": "slack", "key": "k1",
        "session": a_session, "owner": "o"});
    not_yours(&ask(&slack, &acting(A, &squat)));
}

#[test]
fn an_identity_the_socket_may_not_assert_is_refused_even_for_a_list() {
    let world = World::new();
    let list = json!({"cmd": "show"});
    refused(
        &ask(&world.slack(), &acting("web:alice", &list)),
        "may assert only slack:",
    );
}
