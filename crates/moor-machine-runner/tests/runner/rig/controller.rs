//! The fake controller a rig's runner talks to over its socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use moor_core::controller::{ServerFrame, PROTOCOL};
use moor_core::response::{ErrorView, Response};
use moor_core::ErrorKind;
use serde_json::{json, Value};

use super::{FakeHolds, FakeLink, Puts, Targets, MACHINE_ID};

/// What a `machine-hold` claim is answered: the next of `answers`, or slot 0 on minion.
fn hold_answer(holds: &mut FakeHolds, req: &Value, suspended: bool) -> Value {
    holds
        .seen
        .push(json!({"op": req["op"].clone(), "suspended": suspended}));
    if req["op"] == "release" {
        let held = std::mem::take(&mut holds.held);
        return json!({"state": "released", "machine": held.then_some("minion")});
    }
    let next = if holds.answers.len() > 1 {
        holds.answers.pop_front()
    } else {
        holds.answers.front().cloned()
    };
    let answer = next.unwrap_or_else(|| {
        json!({"state": "held", "machine": "minion", "ssh": "moor@minion", "slot": 0,
            "ports": [8081]})
    });
    holds.held |= answer["state"] == "held";
    answer
}

/// The `show` list reply: each Box in `open`, asleep when `asleep` names it.
fn show_value(open: &[i64], asleep: &[i64]) -> Value {
    let boxes: Vec<Value> = open
        .iter()
        .map(|id| {
            let lifecycle = if asleep.contains(id) {
                "asleep"
            } else {
                "ready"
            };
            json!({ "id": id, "lifecycle": lifecycle })
        })
        .collect();
    json!({ "boxes": boxes, "box": null })
}

/// What the fake controller shares with the test.
#[derive(Clone)]
pub(super) struct Fake {
    pub(super) open: Arc<Mutex<Vec<i64>>>,
    pub(super) asleep: Arc<Mutex<Vec<i64>>>,
    pub(super) mirror_excludes: Arc<Mutex<Vec<Value>>>,
    pub(super) puts: Puts,
    pub(super) targets: Targets,
    pub(super) holds: Arc<Mutex<FakeHolds>>,
    pub(super) link: FakeLink,
}

/// Serves `box-mirror` the way the controller does, from `tree`, with a local rsync, and
/// `show` listing the Boxes in `open`.
pub(super) fn fake_controller(socket: &Path, tree: PathBuf, fake: Fake) {
    let listener = UnixListener::bind(socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let (stream, fake, tree) = (stream.unwrap(), fake.clone(), tree.clone());
            std::thread::spawn(move || serve_connection(stream, &tree, &fake));
        }
    });
}

/// Answers one runner connection until it closes.
fn serve_connection(stream: UnixStream, tree: &Path, fake: &Fake) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello = ServerFrame::Hello { protocol: PROTOCOL };
    writeln!(writer, "{}", serde_json::to_string(&hello).unwrap()).unwrap();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let frame: Value = serde_json::from_str(&line).unwrap();
        let id = frame["id"].as_str().unwrap_or("1").to_owned();
        let answer = answer(&frame, tree, fake);
        reply(&mut writer, &id, answer);
    }
}

/// The fake controller's answer to one request `frame`.
fn answer(frame: &Value, tree: &Path, fake: &Fake) -> Response {
    let req = request(frame).unwrap();
    if req["cmd"] == "show" {
        let value = show_value(&fake.open.lock().unwrap(), &fake.asleep.lock().unwrap());
        Response::ok("show", Some(value))
    } else if req["cmd"] == "machine-targets" {
        targets_envelope(fake.targets.lock().unwrap().clone())
    } else if req["cmd"] == "machine-keys" {
        let pubkey = fake.link.pubkey.lock().unwrap().clone();
        let machines: Vec<Value> = pubkey
            .into_iter()
            .map(|pubkey| {
                json!({"machine_id": MACHINE_ID, "user": 1, "name": "minion", "pubkey": pubkey})
            })
            .collect();
        Response::ok("machine-keys", Some(json!({ "machines": machines })))
    } else if req["cmd"] == "machine-links" {
        let sent = req["links"].clone();
        *fake.link.links.lock().unwrap() = Some(sent.clone());
        Response::ok("machine-links", Some(json!({ "links": sent })))
    } else if req["cmd"] == "machine-hold" {
        assert_eq!(req["id"], 7, "{frame}");
        let suspended = tree.with_file_name("remote").join("suspended").exists();
        let value = hold_answer(&mut fake.holds.lock().unwrap(), req, suspended);
        Response::ok("machine-hold", Some(value))
    } else if req["cmd"] == "box-put" {
        let bytes = std::fs::read(req["src"].as_str().unwrap()).unwrap();
        let value = json!({"path": "/work/x", "bytes": bytes.len(), "confirmation": "wrote"});
        fake.puts.lock().unwrap().push((req.clone(), bytes));
        Response::ok("box-put", Some(value))
    } else {
        assert_eq!(req["cmd"], "box-mirror", "{frame}");
        assert_eq!(req["id"], 7, "{frame}");
        fake.mirror_excludes
            .lock()
            .unwrap()
            .push(req["exclude"].clone());
        mirror(tree, req["dest"].as_str().unwrap())
    }
}

/// Serves `box-mirror`: copies `tree` to `dest` with a local rsync.
fn mirror(tree: &Path, dest: &str) -> Response {
    std::fs::create_dir_all(dest).unwrap();
    let ok = Command::new("rsync")
        .args(["-a", "--delete", "--exclude=.git"])
        .arg(format!("{}/", tree.display()))
        .arg(format!("{dest}/"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "fake controller rsync");
    let value = json!({"from": "/work", "to": dest, "files": 1, "bytes": 1,
        "confirmation": "mirrored"});
    Response::ok("box-mirror", Some(value))
}

/// Writes one response frame carrying `envelope`.
fn reply(writer: &mut impl Write, id: &str, envelope: Response) {
    let frame = ServerFrame::Response {
        id: id.to_owned(),
        envelope,
    };
    writeln!(writer, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
}

/// `machine-targets` answered with `targets`, or failed when there are none.
fn targets_envelope(targets: Option<Vec<String>>) -> Response {
    match targets {
        Some(targets) => Response::ok("machine-targets", Some(json!({ "targets": targets }))),
        None => Response::err(
            "machine-targets",
            ErrorView {
                kind: ErrorKind::Internal,
                message: "the store is down".to_owned(),
                hints: Vec::new(),
                context: Vec::new(),
                detail: None,
            },
        ),
    }
}

/// The request inside a client frame: the object that names a `cmd`.
fn request(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(m) if m.contains_key("cmd") => Some(v),
        Value::Object(m) => m.values().find_map(request),
        _ => None,
    }
}
