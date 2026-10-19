use super::*;

#[test]
fn kind_equals_serde_tag_and_payload_has_no_kind() {
    for e in all_events() {
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v.get("kind").and_then(|k| k.as_str()),
            Some(e.kind()),
            "{e:?}"
        );
        let p = e.payload();
        assert!(p.is_object());
        assert!(p.get("kind").is_none(), "{e:?}");
    }
}

#[test]
fn unit_variants_have_empty_payload() {
    for e in [
        Event::Started,
        Event::Working,
        Event::Idled,
        Event::Busy,
        Event::Woke,
        Event::BoxRequested,
        Event::BoxReady,
        Event::BoxAsleep,
        Event::BoxGone,
        Event::BoxDeleted,
    ] {
        assert_eq!(e.payload(), serde_json::json!({}), "{e:?}");
    }
}

#[test]
fn payload_carries_fields() {
    let e = Event::Ran {
        run: Some(RunId::new(7)),
        exit: None,
        interrupted: true,
        error: None,
    };
    assert_eq!(
        e.payload(),
        serde_json::json!({"run": 7, "exit": null, "interrupted": true})
    );
    let e = Event::Ran {
        run: None,
        exit: None,
        interrupted: false,
        error: None,
    };
    assert_eq!(e.kind(), "ran");
    assert_eq!(
        e.payload(),
        serde_json::json!({"run": null, "exit": null, "interrupted": false})
    );
    let e = Event::Ran {
        run: Some(RunId::new(7)),
        exit: None,
        interrupted: false,
        error: Some("busy".to_owned()),
    };
    assert_eq!(
        e.payload(),
        serde_json::json!({"run": 7, "exit": null, "interrupted": false, "error": "busy"})
    );
    let e = Event::Closed {
        from: TaskState::WaitingForBox,
    };
    assert_eq!(e.payload(), serde_json::json!({"from": "waiting_for_box"}));
}

#[test]
fn stays_up_payload_carries_reason() {
    let e = Event::StaysUp { reason: s("busy") };
    assert_eq!(e.kind(), "stays_up");
    assert_eq!(e.payload(), serde_json::json!({"reason": "busy"}));
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind": "stays_up", "reason": "busy"})
    );
    assert!(!e.is_asserted());
}

#[test]
fn is_asserted_matches_list() {
    assert!(Event::Started.is_asserted());
    assert!(!Event::BoxRequested.is_asserted());
    assert!(!Event::Checked {
        credentials: vec![]
    }
    .is_asserted());
    assert!(!Event::RunStarted { run: RunId::new(1) }.is_asserted());
    assert!(!Event::BoxReleaseFailed {
        error: s("c"),
        handle: None
    }
    .is_asserted());
    let n = all_events().iter().filter(|e| e.is_asserted()).count();
    assert_eq!(n, 20);
}

/// The Proxy nonce is a secret; the payload carries exactly these keys.
#[test]
fn proxy_started_payload_has_no_nonce() {
    let e = Event::ProxyStarted {
        box_port: 1,
        host_port: 2,
        pid: 3,
        handle: s("h"),
    };
    let payload = e.payload();
    let mut keys: Vec<_> = payload.as_object().unwrap().keys().cloned().collect();
    keys.sort_unstable();
    assert_eq!(keys, ["box_port", "handle", "host_port", "pid"]);
}
