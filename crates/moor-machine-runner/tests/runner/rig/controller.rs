//! The fake controller a rig's runner talks to over its socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use moor_core::controller::{parse_frame, write_bytes, ClientFrame, ServerFrame, PROTOCOL};
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::{FakeHolds, FakeLink, Rsyncs, MACHINE_ID};

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
    pub(super) rsyncs: Rsyncs,
    /// The runner's state directory, which no request may name.
    pub(super) runner_state: PathBuf,
    pub(super) holds: Arc<Mutex<FakeHolds>>,
    pub(super) link: FakeLink,
    pub(super) show_fails: Arc<AtomicBool>,
    pub(super) shows_refused: Arc<AtomicUsize>,
}

/// Serves `box-rsync` the way the controller does, on `tree` as the Box's work tree, and
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
        let state = fake.runner_state.display().to_string();
        assert!(
            !line.contains(&state),
            "a request named the runner's state: {line}"
        );
        if request(&frame).unwrap()["cmd"] == "box-rsync" {
            let req = request(&frame).unwrap().clone();
            fake.rsyncs.lock().unwrap().push(req.clone());
            rsync(reader, writer, &id, &req, tree);
            return;
        }
        let answer = answer(&frame, tree, fake);
        reply(&mut writer, &id, answer);
    }
}

/// The fake controller's answer to one request `frame`.
fn answer(frame: &Value, tree: &Path, fake: &Fake) -> Response {
    let req = request(frame).unwrap();
    if req["cmd"] == "show" && fake.show_fails.load(Ordering::SeqCst) {
        fake.shows_refused.fetch_add(1, Ordering::SeqCst);
        let error = ErrorView {
            kind: ErrorKind::Failed,
            message: "the store is busy".to_owned(),
            hints: Vec::new(),
            context: Vec::new(),
            detail: None,
        };
        Response::err("show", error)
    } else if req["cmd"] == "show" {
        let value = show_value(&fake.open.lock().unwrap(), &fake.asleep.lock().unwrap());
        Response::ok("show", Some(value))
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
    } else {
        panic!("the fake controller serves no {frame}");
    }
}

/// Runs the runner's `rsync --server` on `tree`, its stdio relayed over the connection as the
/// controller relays it, then answers with its exit code.
fn rsync(
    mut reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    id: &str,
    req: &Value,
    tree: &Path,
) {
    let argv: Vec<String> = serde_json::from_value(req["argv"].clone()).unwrap();
    let (path, head) = argv.split_last().unwrap();
    let path = path.replace('\\', "");
    let mut child = moor_testkit::ChildGuard::new(
        Command::new(&head[0])
            .args(&head[1..])
            .arg(tree.join(path.trim_start_matches('/')))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdin = child.stdin.take();
    let wanted = id.to_owned();
    std::thread::spawn(move || {
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            match parse_frame(line.trim_end()) {
                Ok(ClientFrame::Bytes { id, len }) if id == wanted => {
                    let mut buf = vec![0; len];
                    reader.read_exact(&mut buf).unwrap();
                    if let Some(input) = stdin.as_mut() {
                        let _ = input.write_all(&buf);
                    }
                }
                _ => stdin = None,
            }
            line.clear();
        }
    });
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = out.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        write_bytes(&mut writer, id, &buf[..n]).unwrap();
    }
    let code = child.wait().unwrap().code();
    reply(
        &mut writer,
        id,
        Response::ok("box-rsync", Some(json!({ "exit": code }))),
    );
}

/// Writes one response frame carrying `envelope`.
fn reply(writer: &mut impl Write, id: &str, envelope: Response) {
    let frame = ServerFrame::Response {
        id: id.to_owned(),
        envelope,
    };
    writeln!(writer, "{}", serde_json::to_string(&frame).unwrap()).unwrap();
}

/// The request inside a client frame: the object that names a `cmd`.
fn request(v: &Value) -> Option<&Value> {
    match v {
        Value::Object(m) if m.contains_key("cmd") => Some(v),
        Value::Object(m) => m.values().find_map(request),
        _ => None,
    }
}
