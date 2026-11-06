//! The machine runner's socket: the requests the runner sends, answered as the operator's
//! would be, and nothing else.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde_json::{json, Value};

use super::authz::{every_kind, naming, Names, World, B_BOX};
use super::fronts::{ask, refused, Served};
use super::operator_only::nameless;

/// A Box id no Box has, so the requests that write on the host find nothing to write.
const NO_BOX: i64 = 4242;

/// The command the runner's rsync hands its remote shell to mirror a Box.
const MIRROR: [&str; 6] = [
    "rsync",
    "--server",
    "--sender",
    "-logDtpre.iLsfxCIvu",
    ".",
    "./",
];

fn runners_requests() -> Vec<Value> {
    vec![
        json!({"cmd": "show"}),
        json!({"cmd": "show", "name": B_BOX}),
        json!({"cmd": "machine-keys"}),
        json!({"cmd": "machine-links", "runner": "/run/runner.sock", "links": []}),
        json!({"cmd": "machine-hold", "id": NO_BOX, "op": "release"}),
        json!({"cmd": "box-rsync", "box": NO_BOX, "argv": MIRROR}),
    ]
}

#[test]
fn the_runner_socket_answers_the_runners_requests_as_the_operator_socket_does() {
    let world = World::new();
    let runner = world.served.runner();
    for request in runners_requests() {
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
        "box-rsync",
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

#[test]
fn a_mirror_or_put_by_host_path_is_no_request_on_any_socket() {
    let world = World::new();
    let runner = world.served.runner();
    for request in [
        json!({"cmd": "box-mirror", "id": NO_BOX, "dest": "/m"}),
        json!({"cmd": "box-put", "id": NO_BOX, "src": "/s", "dest": "d"}),
    ] {
        for socket in [&runner, &world.served.main] {
            let got = ask(socket, &request);
            assert_eq!(got["frame"], "error", "{request}: {got}");
            assert!(got.to_string().contains("unknown variant"), "{got}");
        }
    }
}
