//! `list_events` against a fake controller that replays a fixed event log: a fetch from an
//! offset, its filters, its bounded wait, and that repeating a fetch repeats the answer.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::listen::{emit, next, post, token as token_for, wait_live, world as gate, Fake, TIP};
use super::World;

/// A gate over a controller whose log holds events 11 to 13.
fn logged() -> (World, Arc<Mutex<Fake>>, String) {
    let (world, fake) = gate();
    fake.lock().unwrap().log = vec![
        (11, "repo/a".to_owned(), "idled".to_owned()),
        (12, "repo/b".to_owned(), "started".to_owned()),
        (13, "repo/a".to_owned(), "closed".to_owned()),
    ];
    let token = token_for(&world, "moor:read");
    (world, fake, token)
}

/// The tool result of `list_events` with `args`.
fn list(world: &World, token: &str, args: Value) -> Value {
    let params = json!({"name": "list_events", "arguments": args});
    let m = next(&post(world, token, "tools/call", params));
    m["result"].clone()
}

/// The offsets of the events in `result`.
fn offsets(result: &Value) -> Vec<i64> {
    let events = result["structuredContent"]["events"].as_array().unwrap();
    events
        .iter()
        .map(|e| e["offset"].as_i64().unwrap())
        .collect()
}

#[test]
fn a_fetch_returns_the_events_after_the_offset_in_order() {
    let (world, _fake, token) = logged();
    let result = list(&world, &token, json!({"after": 11}));
    assert_eq!(offsets(&result), [12, 13], "{result}");
    let got = &result["structuredContent"];
    assert_eq!(
        (got["next"].as_i64(), got["caught_up"].as_bool()),
        (Some(13), Some(true))
    );
    let first = &got["events"][0];
    assert_eq!(
        (first["kind"].as_str(), first["box"].as_str()),
        (Some("started"), Some("repo/b"))
    );
    for field in ["box_id", "session", "ts", "data"] {
        assert!(first.get(field).is_some(), "{field} in {first}");
    }
}

#[test]
fn the_offset_defaults_to_the_start_of_the_log() {
    let (world, _fake, token) = logged();
    let result = list(&world, &token, json!({}));
    assert_eq!(offsets(&result), [11, 12, 13], "{result}");
}

#[test]
fn a_limit_cuts_the_fetch_and_next_resumes_after_the_last_event() {
    let (world, _fake, token) = logged();
    let result = list(&world, &token, json!({"limit": 2}));
    let got = &result["structuredContent"];
    assert_eq!(offsets(&result), [11, 12], "{result}");
    assert_eq!(
        (got["next"].as_i64(), got["caught_up"].as_bool()),
        (Some(12), Some(false))
    );
    let rest = list(&world, &token, json!({"after": got["next"], "limit": 2}));
    assert_eq!(offsets(&rest), [13], "{rest}");
}

#[test]
fn nothing_past_the_offset_returns_the_caught_up_cursor_never_less_than_after() {
    let (world, _fake, token) = logged();
    let past = list(&world, &token, json!({"after": 13}));
    let got = &past["structuredContent"];
    assert_eq!(
        (
            offsets(&past),
            got["next"].as_i64(),
            got["caught_up"].as_bool()
        ),
        (vec![], Some(13), Some(true))
    );
    let ahead = list(&world, &token, json!({"after": 50}));
    assert_eq!(ahead["structuredContent"]["next"], 50, "{ahead}");

    let (empty_world, _fake) = gate();
    let empty = list(
        &empty_world,
        &token_for(&empty_world, "moor:read"),
        json!({}),
    );
    assert_eq!(
        empty["structuredContent"]["next"], TIP,
        "the cursor saves a rescan"
    );
}

#[test]
fn kinds_and_box_filters_go_to_the_controller_and_narrow_the_events() {
    let (world, fake, token) = logged();
    let result = list(
        &world,
        &token,
        json!({"kinds": ["idled", "closed"], "box": "repo/a"}),
    );
    assert_eq!(offsets(&result), [11, 13], "{result}");
    let watch = fake.lock().unwrap().watches.last().cloned().unwrap();
    assert_eq!(watch["kinds"], json!(["idled", "closed"]));
    assert_eq!(watch["boxes"], json!(["repo/a"]));
    assert_eq!(watch["identity"], "authgear:alice-sub");
    assert_eq!(
        watch["follow"], false,
        "a fetch without a wait does not follow"
    );
}

#[test]
fn the_same_fetch_twice_gives_the_same_answer() {
    let (world, _fake, token) = logged();
    let args = json!({"after": 11, "kinds": ["started", "closed"]});
    let (one, two) = (
        list(&world, &token, args.clone()),
        list(&world, &token, args),
    );
    assert_eq!(one["structuredContent"], two["structuredContent"]);
    assert_eq!(offsets(&one), [12, 13]);
}

#[test]
fn a_wait_returns_the_first_event_that_arrives() {
    let (world, fake, token) = logged();
    let feeder = Arc::clone(&fake);
    let arrival = std::thread::spawn(move || {
        wait_live(&feeder);
        emit(&feeder, 14, "repo/a", "idled");
    });
    let result = list(&world, &token, json!({"after": 13, "wait_seconds": 20}));
    arrival.join().unwrap();
    assert_eq!(offsets(&result), [14], "{result}");
    assert_eq!(result["structuredContent"]["next"], 14);
    let watch = fake.lock().unwrap().watches.last().cloned().unwrap();
    assert!(watch.get("follow").is_none(), "a wait follows the log");
}

#[test]
fn a_wait_with_no_event_returns_empty_when_the_time_is_up() {
    let (world, _fake, token) = logged();
    let started = Instant::now();
    let result = list(&world, &token, json!({"after": 13, "wait_seconds": 1}));
    let got = &result["structuredContent"];
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(
        (
            offsets(&result),
            got["next"].as_i64(),
            got["caught_up"].as_bool()
        ),
        (vec![], Some(13), Some(true))
    );
}

#[test]
fn a_wait_returns_at_once_when_events_are_already_past_the_offset() {
    let (world, _fake, token) = logged();
    let started = Instant::now();
    let result = list(&world, &token, json!({"after": 11, "wait_seconds": 30}));
    assert_eq!(offsets(&result), [12, 13], "{result}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn list_events_needs_the_read_scope() {
    let (world, _fake, _read) = logged();
    let write = token_for(&world, "moor:write");
    let result = list(&world, &write, json!({}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("moor:read"));
}
