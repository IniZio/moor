//! Which 401 and 403 answers from `api.github.com` count as a rejected credential.

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
    let events = drained_events(&rig);
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
    let events = drained_events(&rig);
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
