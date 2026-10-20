//! Which 401 and 403 answers from `api.github.com` count as a rejected credential.

use std::time::Duration;

use super::{query_answered, rig_with, Rig};

/// Kinds and payloads of the Box events recorded after draining `rig`.
fn drained_events(rig: &Rig) -> Vec<(String, String)> {
    rig.live.as_ref().unwrap().drain(&rig.deps(), rig.box_id);
    rig.store
        .events(rig.box_id, 100)
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.payload.to_string()))
        .collect()
}

/// Drains `rig` until a `kind` event is recorded, then once more for the
/// audit nono logs after the client already has the answer.
fn events_after(rig: &Rig, kind: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();
    for _ in 0..100 {
        events = drained_events(rig);
        if events.iter().any(|(k, _)| k == kind) {
            return drained_events(rig);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unreachable!("no {kind} in {events:?}")
}

#[test]
fn anonymous_403_records_no_rejected_credential() {
    let rig = rig_with(Vec::new());
    let (status, _) = query_answered(&rig.client(), "victim", "secrets", Some(403));
    assert_eq!(status, 403);
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(only.authorization.is_none(), "{only:?}");
    let events = events_after(&rig, "credential_withheld");
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert!(kinds.contains(&"credential_withheld"), "{events:?}");
    assert!(!kinds.contains(&"proxy_credential_rejected"), "{events:?}");
    rig.live.unwrap().stop();
}

#[test]
fn credentialed_403_records_a_rejected_credential() {
    let rig = rig_with(Vec::new());
    let (status, _) = query_answered(&rig.client(), "o", "r", Some(403));
    assert_eq!(status, 403);
    let seen = rig.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert!(only.authorization.is_some(), "{only:?}");
    let events = events_after(&rig, "proxy_credential_rejected");
    let rejected: Vec<&str> = events
        .iter()
        .filter(|(k, _)| k == "proxy_credential_rejected")
        .map(|(_, p)| p.as_str())
        .collect();
    let [one] = rejected.as_slice() else {
        unreachable!("one rejection expected: {events:?}")
    };
    assert!(one.contains("403") && one.contains("github-api"), "{one}");
    rig.live.unwrap().stop();
}

#[test]
fn the_filters_own_403_records_no_rejected_credential() {
    let rig = rig_with(Vec::new());
    let mut resp = rig
        .client()
        .post("https://api.github.com/graphql?x=1")
        .header("Authorization", "token placeholder-not-real")
        .header("Content-Type", "application/json")
        .send(r#"{"query":"query{viewer{login}}"}"#)
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);
    let body = resp.body_mut().read_to_string().unwrap();
    assert!(body.contains("non-canonical path"), "{body}");
    assert!(rig.seen().is_empty(), "{:?}", rig.seen());
    let events = events_after(&rig, "egress_refused");
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert!(kinds.contains(&"egress_refused"), "{events:?}");
    assert!(!kinds.contains(&"proxy_credential_rejected"), "{events:?}");
    rig.live.unwrap().stop();
}
