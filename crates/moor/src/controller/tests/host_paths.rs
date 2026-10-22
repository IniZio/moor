//! Requests that name a path on the controller host: only the operator sends them, and the
//! controller never fills one in from its own working directory.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::path::Path;
use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};
use serde_json::{json, Value};

use super::fronts::{ask, refused, Served};
use crate::store::Store;
use crate::testing::clock::FakeClock;

const ANN: &str = "slack:T1:ANN";
pub(super) const BOX: &str = "ann-box";

/// A crabbox that reports a supported version and copies by writing `copied` to the last
/// argument.
const FAKE_CRABBOX: &str = "#!/bin/sh\n\
    case \"$1\" in\n\
    --version) echo 'crabbox 0.79.1-moor.1' ;;\n\
    cp) for a; do last=$a; done; printf copied > \"$last\" ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// A controller whose crabbox is [`FAKE_CRABBOX`], and its store.
fn served() -> (Served, Store) {
    let dir = tempfile::tempdir().expect("tmp");
    let crabbox = dir.path().join("crabbox");
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).expect("fake crabbox");
    let served = Served::start_in(
        dir,
        &[("MOOR_CRABBOX_BIN", crabbox.to_str().expect("utf8"))],
    );
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let db = served.dir.path().join("state").join("moor.db");
    let store = Store::open(&db, clock).expect("store");
    (served, store)
}

/// `request`, run as `identity`.
fn acting(identity: &str, request: &Value) -> Value {
    json!({"cmd": "as", "identity": identity, "request": request})
}

/// Enrolls `identity` through the `slack` front and gives its user a `claude` secret.
fn enroll_ann(served: &Served) -> UserId {
    let slack = served.front("slack");
    let got = ask(&slack, &json!({"cmd": "enroll", "identity": ANN}));
    let set = json!({"cmd": "secret-set", "identity": ANN, "name": "claude", "value": "x"});
    assert_eq!(ask(&slack, &set)["class"], "ok");
    UserId::new(got["value"]["user"].as_i64().expect("user"))
}

/// Inserts a Box of `user` whose machine is ready, working in `worktree`.
pub(super) fn ready_box(store: &Store, user: UserId, worktree: &Path) -> BoxId {
    let new = Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(BOX).expect("name"),
        repo: "r".into(),
    })
    .requester(user);
    let id = store.apply(new).expect("box").box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        worktree: Field::Set(worktree.display().to_string()),
        ..Fields::default()
    };
    let admitted = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(admitted).expect("admit");
    let ready = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("p/ann"),
        });
    store.apply(ready).expect("ready");
    id
}

fn copies(store: &Store, id: BoxId) -> usize {
    let events = store.events(id, 100).expect("events");
    events.iter().filter(|e| e.kind == "copied").count()
}

#[test]
fn a_front_cp_to_the_host_is_refused_and_the_operators_cp_still_copies() {
    let (served, store) = served();
    let user = enroll_ann(&served);
    let id = ready_box(&store, user, served.dir.path());
    let dest = served.dir.path().join("out.txt");
    let cp = json!({"cmd": "cp", "box": BOX, "src": "/x", "dest": dest});
    let got = ask(&served.front("slack"), &acting(ANN, &cp));
    refused(&got, "cp to the host is operator-only");
    assert!(!dest.exists(), "the refused cp wrote {}", dest.display());
    assert_eq!(copies(&store, id), 0);
    let got = ask(&served.main, &cp);
    assert_eq!(got["class"], "ok", "{got}");
    assert_eq!(std::fs::read_to_string(&dest).expect("copied"), "copied");
    assert_eq!(copies(&store, id), 1);
}

#[test]
fn a_front_cannot_name_a_host_directory() {
    let (served, _store) = served();
    enroll_ann(&served);
    let inside = json!({"cmd": "in", "dir": "/", "request": {"cmd": "env"}});
    let got = ask(&served.front("slack"), &acting(ANN, &inside));
    refused(&got, "in names a host directory, which is operator-only");
}

/// The test process runs inside this crate's checkout, which a start from the controller's own
/// directory would use.
#[test]
fn a_start_with_no_repo_and_no_dir_starts_nothing() {
    let (served, store) = served();
    enroll_ann(&served);
    let operator = json!({"cmd": "secret-set", "name": "claude", "value": "x"});
    ask(&served.main, &json!({"cmd": "enroll"}));
    assert_eq!(ask(&served.main, &operator)["class"], "ok");
    let start = json!({"cmd": "start", "name": "demo"});
    for got in [
        ask(&served.front("slack"), &acting(ANN, &start)),
        ask(&served.main, &start),
    ] {
        assert_eq!(got["error"]["kind"], "usage", "{got}");
        let message = got["error"]["message"].as_str().unwrap_or_default();
        assert!(message.starts_with("no repo"), "{got}");
    }
    let demo = BoxName::parse_allow_bare("demo").expect("name");
    assert!(store.latest_box_by_name(&demo).expect("read").is_none());
}
