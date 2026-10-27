//! End to end: moor-web against a real `moor controller`, with no provider.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code: a failed setup step should abort the test; indexing a JSON value yields null, not a panic"
)]

mod support;

use std::fs::{self, File};
use std::io::Write;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::Stack;

/// Parses a JSON reply body.
fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not json ({e}): {body}"))
}

/// The root in `/tree` whose request carries `key`.
fn root_for(stack: &Stack, key: &str) -> Option<Value> {
    let (status, body) = stack.get("/tree");
    assert_eq!(status, 200, "GET /tree: {body}\n{}", stack.diagnostics());
    parse(&body)["roots"]
        .as_array()?
        .iter()
        .find(|r| r["request"]["key"] == key)
        .cloned()
}

/// Posts a new request and returns its key after checking the job failed only at `start`.
fn create_request(stack: &Stack) -> String {
    let (status, body) = stack.post(
        "/requests",
        &json!({"repo": "o/demo", "name": "t1", "prompt": "g"}),
    );
    assert_eq!(
        status,
        202,
        "POST /requests: {body}\n{}",
        stack.diagnostics()
    );
    let accepted = parse(&body);
    let key = accepted["key"].as_str().expect("key").to_owned();
    let job = accepted["job"].as_str().expect("job").to_owned();

    // Nothing before the worker start can fail.
    let snap = stack.wait_job(&job);
    let diag = stack.diagnostics();
    assert_eq!(snap["state"], "failed", "job: {snap}\n{diag}");
    assert_eq!(snap["failure"]["step"], "start", "job: {snap}\n{diag}");
    let message = snap["failure"]["message"].as_str().unwrap_or_default();
    assert!(!message.is_empty(), "job: {snap}\n{diag}");
    let progress = snap["progress"].to_string();
    for line in [
        "creating the request workdir",
        "opening the request session",
        "binding the request",
        "saving the request",
        "starting the worker",
    ] {
        assert!(progress.contains(line), "no `{line}` in {progress}");
    }
    key
}

/// Checks the open root in the tree, the binding and the Session; returns the root id.
fn check_open_root(stack: &Stack, key: &str) -> i64 {
    let diag = stack.diagnostics();
    let root = root_for(stack, key).unwrap_or_else(|| panic!("no root for key\n{diag}"));
    let id = root["session"].as_i64().expect("root session id");
    assert_eq!(root["name"], Value::Null, "root has a Box: {root}");
    assert_eq!(root["request"]["repo"], "o/demo");
    assert_eq!(root["request"]["ended_at"], Value::Null);
    assert_ne!(root["state"], "gone", "root: {root}");

    let bound = stack.controller(json!({"cmd": "session-binding", "front": "web", "key": key}));
    let binding = &bound["value"]["binding"];
    let bound_session = if binding.is_null() {
        &bound["value"]["session"]
    } else {
        &binding["session"]
    };
    assert_eq!(bound_session, &json!(id), "binding: {bound}\n{diag}");

    let shown = stack.controller(json!({"cmd": "show-session", "session": id}));
    assert_eq!(shown["value"]["box"], Value::Null, "show: {shown}");
    assert_eq!(shown["value"]["ended_at"], Value::Null, "show: {shown}");
    id
}

/// Ends the request and checks the root ends, the tree shows it gone and workers are refused.
fn end_request(stack: &Stack, key: &str, id: i64) {
    let (status, body) = stack.post(&format!("/requests/{key}/end"), &json!({}));
    assert_eq!(status, 202, "end: {body}\n{}", stack.diagnostics());
    let end_job = parse(&body)["job"].as_str().expect("job").to_owned();
    let snap = stack.wait_job(&end_job);
    let diag = stack.diagnostics();
    assert_eq!(snap["state"], "done", "end job: {snap}\n{diag}");
    assert_eq!(snap["result"]["root_ended"], true, "end job: {snap}");

    let shown = stack.controller(json!({"cmd": "show-session", "session": id}));
    let listed = stack.controller(json!({"cmd": "sessions"}));
    let still_live = listed["value"]["sessions"]
        .as_array()
        .is_some_and(|s| s.iter().any(|r| r["session"] == json!(id)));
    assert!(
        shown["value"]["ended_at"].is_i64() || !still_live,
        "root still live: {shown}\n{diag}"
    );

    let gone = root_for(stack, key).unwrap_or_else(|| panic!("root vanished\n{diag}"));
    assert_eq!(gone["state"], "gone", "root: {gone}\n{diag}");
    assert!(gone["request"]["ended_at"].is_i64(), "unset: {gone}");

    let (status, body) = stack.post(
        &format!("/requests/{key}/workers"),
        &json!({"name": "t2", "prompt": "g"}),
    );
    assert_eq!(status, 409, "workers after end: {body}\n{diag}");
}

#[test]
fn request_lifecycle_against_a_real_controller() {
    let stack = Stack::start();
    let (status, page) = stack.get("/");
    assert_eq!(status, 200, "GET /\n{}", stack.diagnostics());
    assert!(page.contains("/app.js"), "index lacks /app.js");

    let key = create_request(&stack);
    let id = check_open_root(&stack, &key);
    end_request(&stack, &key, id);
}

#[test]
fn a_repo_that_is_not_owner_name_is_refused_before_any_root_is_opened() {
    let stack = Stack::start();
    let (status, body) = stack.post(
        "/requests",
        &json!({"repo": "demo", "name": "t1", "prompt": "g"}),
    );
    assert_eq!(status, 400, "POST /requests: {body}");

    let listed = stack.controller(json!({"cmd": "sessions"}));
    let sessions = listed["value"]["sessions"].as_array().expect("sessions");
    assert!(
        sessions.is_empty(),
        "sessions: {listed}\n{}",
        stack.diagnostics()
    );
}

/// Size of the large artifact.
const BIG: u64 = 500 * 1024 * 1024;

/// Length of the repeating test pattern; not a multiple of a bytes frame.
const PERIOD: usize = 1024 * 1024 + 13;

/// Most a process may grow while it moves the large artifact, in KiB.
const GROWTH_KIB: u64 = 16 * 1024;

/// The pattern the large artifact repeats.
fn pattern() -> Vec<u8> {
    (0..PERIOD)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect()
}

/// Whether `piece`, found at `offset` of the large artifact, holds the pattern's bytes.
fn in_pattern(block: &[u8], offset: u64, piece: &[u8]) -> bool {
    let start = usize::try_from(offset % PERIOD as u64).unwrap();
    let first = (PERIOD - start).min(piece.len());
    piece[..first] == block[start..start + first] && piece[first..] == block[..piece.len() - first]
}

/// Stores `big.bin` of `size` pattern bytes for a Box named `big` the way a collect leaves it:
/// a row in the controller's database and the file under its state directory.
fn publish_pattern(stack: &Stack, block: &[u8], size: u64) {
    stack.controller(json!({"cmd": "sessions"}));
    let state = stack.state_dir();
    let db = rusqlite::Connection::open(state.join("moor.db")).unwrap();
    db.execute(
        "INSERT INTO boxes (name, repo, state, created_at, updated_at) \
         VALUES ('big', 'o/demo', 'ready', 0, 0)",
        [],
    )
    .unwrap();
    let id = db.last_insert_rowid();
    let dir = state.join("artifacts").join(id.to_string()).join("v1");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big.bin");
    let mut file = File::create(&path).unwrap();
    let mut digest = Sha256::new();
    let mut left = size;
    while left > 0 {
        let n = usize::try_from(left.min(PERIOD as u64)).unwrap();
        file.write_all(&block[..n]).unwrap();
        digest.update(&block[..n]);
        left -= n as u64;
    }
    db.execute(
        "INSERT INTO artifacts (box, name, version, kind, content_type, size, sha256, path, \
         created_at) VALUES (?1, 'big.bin', 1, 'file', 'application/octet-stream', ?2, ?3, ?4, 0)",
        rusqlite::params![
            id,
            i64::try_from(size).unwrap(),
            format!("{:x}", digest.finalize()),
            path.to_str().unwrap()
        ],
    )
    .unwrap();
}

/// The value of `key` in `/proc/<pid>/status`, in KiB.
fn status_kib(pid: u32, key: &str) -> u64 {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or_else(|| panic!("no {key} for pid {pid}"))
}

#[test]
#[allow(clippy::print_stderr, reason = "the RSS samples are the test's record")]
fn a_500_mib_artifact_streams_through_in_bounded_memory() {
    let stack = Stack::start();
    let block = pattern();
    publish_pattern(&stack, &block, BIG);
    let path = "/boxes/big/artifacts/big.bin";
    let (controller, web) = stack.pids();
    let before = [controller, web].map(|pid| {
        fs::write(format!("/proc/{pid}/clear_refs"), "5").unwrap();
        status_kib(pid, "VmRSS")
    });
    let mut got = 0;
    let head = stack.stream(path, "", |offset, piece| {
        assert!(in_pattern(&block, offset, piece), "wrong bytes at {offset}");
        got += piece.len() as u64;
    });
    let peak = [controller, web].map(|pid| status_kib(pid, "VmHWM"));
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "{head}\n{}",
        stack.diagnostics()
    );
    assert_eq!(got, BIG, "{}", stack.diagnostics());
    for (who, (b, p)) in ["controller", "moor-web"]
        .iter()
        .zip(before.iter().zip(peak))
    {
        eprintln!(
            "{who}: rss before {b} KiB, peak during {p} KiB, growth {} KiB",
            p - b
        );
        assert!(p - b <= GROWTH_KIB, "{who} grew {} KiB", p - b);
    }
}

#[test]
fn ranges_of_a_real_artifact_are_206_or_416() {
    const SIZE: u64 = 3 * 1024 * 1024 + 7;
    let stack = Stack::start();
    let block = pattern();
    publish_pattern(&stack, &block, SIZE);
    let path = "/boxes/big/artifacts/big.bin";
    let mut tail = Vec::new();
    let head = stack.stream(path, "Range: bytes=-1000\r\n", |_, piece| {
        tail.extend_from_slice(piece);
    });
    assert!(
        head.starts_with("HTTP/1.1 206"),
        "{head}\n{}",
        stack.diagnostics()
    );
    let range = format!("Content-Range: bytes {}-{}/{SIZE}", SIZE - 1000, SIZE - 1);
    assert!(head.contains(&range), "{head}");
    assert!(in_pattern(&block, SIZE - 1000, &tail) && tail.len() == 1000);
    let head = stack.stream(path, &format!("Range: bytes={SIZE}-\r\n"), |_, _| {});
    assert!(head.starts_with("HTTP/1.1 416"), "{head}");
    assert!(
        head.contains(&format!("Content-Range: bytes */{SIZE}")),
        "{head}"
    );
}
