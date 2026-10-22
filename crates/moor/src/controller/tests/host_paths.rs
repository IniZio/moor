//! Requests that name a path on the controller host: only the operator sends them, and no request
//! names a directory for the controller to read.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

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

/// A crabbox that reports a supported version and the ssh target of every Box.
const FAKE_CRABBOX: &str = "#!/bin/sh\n\
    case \"$1\" in\n\
    --version) echo 'crabbox 0.79.1-moor.1' ;;\n\
    ssh) echo 'ssh -o StrictHostKeyChecking=yes u@box' ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// An ssh that runs the remote command, its last argument, on this host.
const FAKE_SSH: &str = "#!/bin/sh\nfor last; do :; done\nexec sh -c \"$last\"\n";

/// A controller whose crabbox is [`FAKE_CRABBOX`] and ssh [`FAKE_SSH`], and its store.
fn served() -> (Served, Store) {
    let dir = tempfile::tempdir().expect("tmp");
    let crabbox = dir.path().join("crabbox");
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).expect("fake crabbox");
    let ssh = dir.path().join("ssh");
    moor_testkit::write_executable(&ssh, FAKE_SSH).expect("fake ssh");
    let served = Served::start_in(
        dir,
        &[
            ("MOOR_CRABBOX_BIN", crabbox.to_str().expect("utf8")),
            ("MOOR_SSH_BIN", ssh.to_str().expect("utf8")),
        ],
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

/// Inserts a Box of `user` whose machine is ready.
pub(super) fn ready_box(store: &Store, user: UserId) -> BoxId {
    let new = Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(BOX).expect("name"),
        repo: None,
    })
    .requester(user);
    let id = store.apply(new).expect("box").box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
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
    let id = ready_box(&store, user);
    let src = served.dir.path().join("in-box.txt");
    std::fs::write(&src, "copied").expect("box file");
    let dest = served.dir.path().join("out.txt");
    let cp = json!({"cmd": "cp", "box": BOX, "src": src, "dest": dest});
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
fn no_request_names_a_host_directory_for_the_controller_to_read() {
    let (served, _store) = served();
    enroll_ann(&served);
    let inside = json!({"cmd": "in", "dir": "/", "request": {"cmd": "env"}});
    for got in [
        ask(&served.front("slack"), &acting(ANN, &inside)),
        ask(&served.main, &inside),
    ] {
        assert_eq!(got["frame"], "error", "{got}");
    }
}
