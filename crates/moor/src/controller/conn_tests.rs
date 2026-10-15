#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index decoded frames"
)]
use std::io::Cursor;

use serde_json::{json, Value};

use super::{handle, respond};
use crate::env::Env;
use crate::Moor;
use moor_core::name::TaskName;
use moor_core::{Request, ShowReq};

fn env(dir: &tempfile::TempDir) -> Env {
    Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().expect("utf8 path")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ])
}

/// Feed `lines` through `handle` and return the frames written back.
fn run(env: &Env, lines: &[Value]) -> Vec<Value> {
    let input = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    run_raw(env, &input)
}

fn run_raw(env: &Env, input: &str) -> Vec<Value> {
    let mut out = Vec::new();
    handle(Cursor::new(input.as_bytes().to_vec()), &mut out, env);
    String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json frame"))
        .collect()
}

fn show(task: Option<TaskName>) -> Request {
    Request::Show(ShowReq {
        task,
        events: None,
        agent: false,
        reports: false,
        pane: false,
    })
}

fn hello() -> Value {
    json!({"frame": "hello", "protocol": 1})
}

fn request(id: &str, request: &Request) -> Value {
    json!({"frame": "request", "id": id, "request": request})
}

#[test]
fn hello_then_version_echoes_id_and_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let frames = run(&env(&dir), &[hello(), request("r1", &Request::Version {})]);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], json!({"frame": "hello", "protocol": 1}));
    assert_eq!(frames[1]["frame"], "response");
    assert_eq!(frames[1]["id"], "r1");
    let expected = serde_json::to_value(Moor::version()).expect("value");
    assert_eq!(frames[1]["envelope"], expected);
}

#[test]
fn envelopes_match_moor_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env = env(&dir);
    let unknown = TaskName::parse("nope").expect("name");
    let requests = [
        Request::Version {},
        show(None),
        show(Some(unknown)),
        Request::ProxyCheck {},
    ];
    for (i, req) in requests.iter().enumerate() {
        let id = format!("r{i}");
        let frames = run(&env, &[hello(), request(&id, req)]);
        let expected = serde_json::to_value(respond(&env, req.clone())).expect("value");
        assert_eq!(frames[1]["id"], id.as_str());
        assert_eq!(frames[1]["envelope"], expected, "{req:?}");
    }
}

#[test]
fn unknown_task_gives_error_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let unknown = TaskName::parse("nope").expect("name");
    let req = show(Some(unknown));
    let frames = run(&env(&dir), &[hello(), request("x", &req)]);
    assert_eq!(frames[1]["frame"], "response");
    assert!(frames[1]["envelope"]["error"].is_object());
}

#[test]
fn wrong_protocol_is_rejected_then_session_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = json!({"frame": "hello", "protocol": 99});
    let frames = run(
        &env(&dir),
        &[bad, hello(), request("r1", &Request::Version {})],
    );
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["frame"], "error");
    assert_eq!(frames[0]["kind"], "unsupported-protocol");
    let message = frames[0]["message"].as_str().expect("message");
    assert!(message.contains("99") && message.contains('1'), "{message}");
    assert_eq!(frames[1]["frame"], "hello");
    assert_eq!(frames[2]["frame"], "response");
}

#[test]
fn request_after_rejected_hello_still_needs_hello() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = json!({"frame": "hello", "protocol": 99});
    let frames = run(&env(&dir), &[bad, request("r1", &Request::Version {})]);
    assert_eq!(frames[1]["kind"], "hello-required");
}

#[test]
fn malformed_frames_keep_the_connection_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = concat!(
        "{not json\n",
        "\n",
        "{\"frame\":\"hello\",\"protocol\":1,\"extra\":true}\n",
        "{\"frame\":\"hello\",\"protocol\":1}\n",
        "{\"frame\":\"request\",\"id\":\"r1\",\"request\":{\"cmd\":\"version\"}}\n",
    );
    let frames = run_raw(&env(&dir), input);
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0]["kind"], "malformed");
    assert_eq!(frames[1]["kind"], "malformed");
    assert_eq!(frames[2]["frame"], "hello");
    assert_eq!(frames[3]["id"], "r1");
}

#[test]
fn request_before_hello_is_rejected_with_its_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let frames = run(&env(&dir), &[request("early", &Request::Version {})]);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["frame"], "error");
    assert_eq!(frames[0]["kind"], "hello-required");
    assert_eq!(frames[0]["id"], "early");
}
