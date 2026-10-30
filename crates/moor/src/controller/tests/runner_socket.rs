//! The machine runner's socket: the requests the runner sends, answered as the operator's
//! would be, and nothing else.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::authz::{every_kind, naming, Names, World, B_BOX};
use super::fronts::{ask, refused, Served};
use super::operator_only::nameless;

/// A Box id no Box has, so the requests that write on the host find nothing to write.
const NO_BOX: i64 = 4242;

/// The machine runner's own state directory in `world`, made as the runner makes it.
fn runner_state(world: &World) -> PathBuf {
    let dir = world.served.dir.path().join("state").join("machine-runner");
    std::fs::create_dir_all(dir.join("pulls")).expect("runner state");
    dir
}

fn runners_requests(own: &Path) -> Vec<Value> {
    let mirror = own.join("mirrors").join(NO_BOX.to_string());
    let staged = own.join("pulls").join("1-0");
    vec![
        json!({"cmd": "show"}),
        json!({"cmd": "show", "name": B_BOX}),
        json!({"cmd": "machine-keys"}),
        json!({"cmd": "machine-links", "links": []}),
        json!({"cmd": "machine-hold", "id": NO_BOX, "op": "release"}),
        json!({"cmd": "box-mirror", "id": NO_BOX, "dest": mirror}),
        json!({"cmd": "box-put", "id": NO_BOX, "src": staged, "dest": "d"}),
    ]
}

#[test]
fn the_runner_socket_answers_the_runners_requests_as_the_operator_socket_does() {
    let world = World::new();
    let runner = world.served.runner();
    for request in runners_requests(&runner_state(&world)) {
        let got = ask(&runner, &request);
        assert_eq!(got, ask(&world.served.main, &request), "{request}");
    }
}

#[test]
fn the_runner_socket_refuses_every_other_request_but_version() {
    let world = World::new();
    let runner = world.served.runner();
    // A version is the handshake every socket answers.
    let theirs = [
        "version",
        "show",
        "machine-hold",
        "machine-keys",
        "machine-links",
        "box-mirror",
        "box-put",
    ];
    let mut unknown = Vec::new();
    for kind in every_kind()
        .iter()
        .filter(|k| !theirs.contains(&k.as_str()) && *k != "as")
    {
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
        refused(
            &ask(&runner, &request),
            "the machine runner's socket serves only",
        );
    }
    assert!(
        unknown.is_empty(),
        "say how to send these new request kinds: {unknown:?}"
    );
}

#[test]
fn the_runner_socket_is_owner_only_in_an_owner_only_dir() {
    let served = Served::start();
    let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    let runner = served.runner();
    assert_eq!(mode(&runner), 0o600);
    assert_eq!(mode(runner.parent().expect("dir")), 0o700);
}

/// The host paths a runner may not name: the controller's own files, a `..` out, a symlink out.
fn not_the_runners_paths(world: &World) -> Vec<String> {
    let own = runner_state(world);
    let state = world.served.dir.path().join("state");
    symlink(&state, own.join("escape")).expect("link");
    [
        state.join("moor.db"),
        PathBuf::from("/etc/passwd"),
        own.join("..").join("moor.db"),
        own.join("escape").join("moor.db"),
    ]
    .iter()
    .map(|p| p.display().to_string())
    .collect()
}

#[test]
fn the_runner_socket_refuses_host_paths_outside_the_runners_state_dir() {
    let world = World::new();
    let runner = world.served.runner();
    for path in not_the_runners_paths(&world) {
        for request in [
            json!({"cmd": "box-mirror", "id": NO_BOX, "dest": path}),
            json!({"cmd": "box-put", "id": NO_BOX, "src": path, "dest": "d"}),
        ] {
            let got = ask(&runner, &request);
            assert_eq!(got["error"]["kind"], "refused", "{request}: {got}");
        }
    }
}

#[test]
fn the_operator_socket_still_names_any_host_path() {
    let world = World::new();
    for path in not_the_runners_paths(&world) {
        let got = ask(
            &world.served.main,
            &json!({"cmd": "box-mirror", "id": NO_BOX, "dest": path}),
        );
        assert_ne!(got["error"]["kind"], "refused", "{path}: {got}");
    }
}

#[test]
fn a_configured_runner_state_dir_replaces_the_default() {
    let dir = tempfile::tempdir().expect("tmp");
    let own = dir.path().join("elsewhere");
    std::fs::create_dir_all(&own).expect("own");
    let served = Served::start_fronts(dir, &format!("runner_state_dir: {}\n", own.display()), &[]);
    let default = served.dir.path().join("state").join("machine-runner");
    std::fs::create_dir_all(&default).expect("default");
    let mirror = |root: &Path| {
        let dest = root.join("mirrors").join(NO_BOX.to_string());
        ask(
            &served.runner(),
            &json!({"cmd": "box-mirror", "id": NO_BOX, "dest": dest}),
        )
    };
    let got = mirror(&own);
    assert_ne!(got["error"]["kind"], "refused", "{got}");
    refused(&mirror(&default), "is outside");
}
