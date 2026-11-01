//! `GET /events`: `list_events` over plain HTTP, with an Authgear token.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::listen::{emit, token as token_for, wait_live, world as gate, Fake};
use super::World;

/// What a `GET /events` got back.
struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    text: String,
}

impl Got {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn offsets(&self) -> Vec<i64> {
        let doc = self.json();
        let events = doc["events"].as_array().unwrap();
        events
            .iter()
            .map(|e| e["offset"].as_i64().unwrap())
            .collect()
    }
}

/// `GET /events?query` on the gate, with `token` when given.
fn get(world: &World, token: Option<&str>, query: &str) -> Got {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(format!("http://{}/events?{query}", world.gate));
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let mut res = req.call().unwrap();
    Got {
        status: res.status().as_u16(),
        headers: res
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
            .collect(),
        text: res.body_mut().read_to_string().unwrap(),
    }
}

/// A gate over a controller whose log holds events 11 to 13.
fn logged() -> (World, Arc<Mutex<Fake>>) {
    let (world, fake) = gate();
    fake.lock().unwrap().log = vec![
        (11, "repo/a".to_owned(), "idled".to_owned()),
        (12, "repo/b".to_owned(), "started".to_owned()),
        (13, "repo/a".to_owned(), "closed".to_owned()),
    ];
    (world, fake)
}

#[test]
fn an_authgear_token_reads_events_as_json_that_is_never_cached() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    let got = get(&world, Some(&token), "after=11");
    assert_eq!(got.status, 200, "{}", got.text);
    assert_eq!(got.header("content-type"), Some("application/json"));
    assert_eq!(got.header("cache-control"), Some("no-store"));
    assert_eq!(got.offsets(), [12, 13]);
    let doc = got.json();
    assert_eq!(
        (doc["next"].as_i64(), doc["caught_up"].as_bool()),
        (Some(13), Some(true))
    );
    let first = &doc["events"][0];
    assert_eq!(
        (first["kind"].as_str(), first["box"].as_str()),
        (Some("started"), Some("repo/b"))
    );
    for field in ["box_id", "session", "ts", "data"] {
        assert!(first.get(field).is_some(), "{field} in {first}");
    }
}

#[test]
fn repeated_kind_and_box_narrow_the_events_with_percent_decoding() {
    let (world, fake) = logged();
    let token = token_for(&world, "moor:read");
    let got = get(&world, Some(&token), "kind=idled&kind=closed&box=repo%2Fa");
    assert_eq!(got.offsets(), [11, 13], "{}", got.text);
    let watch = fake.lock().unwrap().watches.last().cloned().unwrap();
    assert_eq!(watch["kinds"], json!(["idled", "closed"]));
    assert_eq!(watch["boxes"], json!(["repo/a"]));
}

#[test]
fn a_limit_cuts_the_fetch_and_next_resumes() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    let got = get(&world, Some(&token), "limit=2");
    assert_eq!(got.offsets(), [11, 12]);
    let doc = got.json();
    assert_eq!(
        (doc["next"].as_i64(), doc["caught_up"].as_bool()),
        (Some(12), Some(false))
    );
    assert_eq!(
        get(&world, Some(&token), "after=12&limit=2").offsets(),
        [13]
    );
}

#[test]
fn the_same_request_twice_gives_the_same_body() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    let one = get(&world, Some(&token), "after=11&kind=started&kind=closed");
    let two = get(&world, Some(&token), "after=11&kind=started&kind=closed");
    assert_eq!(one.text, two.text);
    assert_eq!(one.offsets(), [12, 13]);
}

#[test]
fn a_missing_or_bad_token_gets_401_with_the_challenge() {
    let (world, _fake) = logged();
    for token in [None, Some("not-a-jwt")] {
        let got = get(&world, token, "");
        assert_eq!(got.status, 401, "{token:?}: {}", got.text);
        let challenge = got.header("www-authenticate").unwrap();
        assert!(challenge.contains("resource_metadata="), "{challenge}");
        assert!(got.json()["error"].is_string(), "{}", got.text);
    }
}

#[test]
fn a_token_without_the_read_scope_gets_403() {
    let (world, _fake) = logged();
    let write = token_for(&world, "moor:write");
    let got = get(&world, Some(&write), "");
    assert_eq!(got.status, 403, "{}", got.text);
    assert!(got.json()["error"].as_str().unwrap().contains("moor:read"));
}

#[test]
fn a_bad_parameter_gets_400_naming_it() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    for name in ["after", "limit", "wait_seconds"] {
        let got = get(&world, Some(&token), &format!("{name}=soon"));
        assert_eq!(got.status, 400, "{name}: {}", got.text);
        let why = got.json()["error"].as_str().unwrap().to_owned();
        assert!(why.contains(name), "{why}");
    }
    let got = get(&world, Some(&token), "box=");
    assert_eq!(got.status, 400, "{}", got.text);
    assert!(got.json()["error"].as_str().unwrap().contains("box"));
}

#[test]
fn a_wait_returns_the_first_event_that_arrives() {
    let (world, fake) = logged();
    let token = token_for(&world, "moor:read");
    let feeder = Arc::clone(&fake);
    let arrival = std::thread::spawn(move || {
        wait_live(&feeder);
        emit(&feeder, 14, "repo/a", "idled");
    });
    let got = get(&world, Some(&token), "after=13&wait_seconds=20");
    arrival.join().unwrap();
    assert_eq!(got.offsets(), [14], "{}", got.text);
    assert_eq!(got.json()["next"], 14);
}

#[test]
fn a_wait_with_no_event_returns_empty_when_the_time_is_up() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    let started = Instant::now();
    let got = get(&world, Some(&token), "after=13&wait_seconds=1");
    assert!(started.elapsed() >= Duration::from_millis(900));
    let doc = got.json();
    assert_eq!(
        (
            got.offsets(),
            doc["next"].as_i64(),
            doc["caught_up"].as_bool()
        ),
        (vec![], Some(13), Some(true))
    );
}

#[test]
fn a_wait_returns_at_once_when_events_are_already_past_the_offset() {
    let (world, _fake) = logged();
    let token = token_for(&world, "moor:read");
    let started = Instant::now();
    let got = get(&world, Some(&token), "after=11&wait_seconds=30");
    assert_eq!(got.offsets(), [12, 13]);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_route_is_404_when_the_gate_does_not_serve_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let (_unused, rx) = std::sync::mpsc::channel();
    let world = super::world_with(socket, rx, dir, false);
    let got = get(&world, Some("whatever"), "");
    assert_eq!(got.status, 404);
}
