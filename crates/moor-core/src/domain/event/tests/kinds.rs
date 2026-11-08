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
        from: BoxLifecycle::Pending,
        reason: None,
    };
    assert_eq!(e.payload(), serde_json::json!({"from": "pending"}));
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

/// The payload carries exactly these keys.
#[test]
fn proxy_started_payload_has_exactly_its_keys() {
    let e = Event::ProxyStarted {
        box_port: 1,
        pid: 3,
        handle: s("h"),
    };
    let payload = e.payload();
    let mut keys: Vec<_> = payload.as_object().unwrap().keys().cloned().collect();
    keys.sort_unstable();
    assert_eq!(keys, ["box_port", "handle", "pid"]);
}

#[test]
fn every_event_kind_is_listed_once() {
    for e in all_events() {
        assert!(EVENT_KINDS.contains(&e.kind()), "{e:?}");
    }
    let mut seen = std::collections::BTreeSet::new();
    for k in EVENT_KINDS {
        assert!(seen.insert(k), "{k} is listed twice");
    }
}

#[test]
fn machine_events_carry_their_own_kind() {
    let e = Event::Machine(MachineEvent::MachineForwardFailed {
        port: Some(8081),
        error: "refused".to_owned(),
    });
    assert_eq!(e.kind(), "machine_forward_failed");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind": "machine_forward_failed", "port": 8081, "error": "refused"})
    );
    assert_eq!(
        e.payload(),
        serde_json::json!({"port": 8081, "error": "refused"})
    );
}

/// `MachineWaiting` carries the place in line and `MachineRefused` only its reason.
#[test]
fn machine_waiting_and_refused_payloads() {
    let waiting = Event::Machine(MachineEvent::MachineWaiting {
        name: "mini".to_owned(),
        position: 2,
        reason: "full".to_owned(),
    });
    assert_eq!(waiting.kind(), "machine_waiting");
    assert_eq!(
        waiting.payload(),
        serde_json::json!({"name": "mini", "position": 2, "reason": "full"})
    );
    let refused = Event::Machine(MachineEvent::MachineRefused {
        reason: "no".to_owned(),
    });
    assert_eq!(refused.kind(), "machine_refused");
    assert_eq!(refused.payload(), serde_json::json!({"reason": "no"}));
}

#[test]
fn stall_events_carry_their_own_kind() {
    let stalled = Event::Stall(StallEvent::AgentStalled {
        doing: Some("Bash sleep".to_owned()),
        quiet_secs: 700,
    });
    assert_eq!(stalled.kind(), "agent_stalled");
    assert_eq!(
        serde_json::to_value(&stalled).unwrap(),
        serde_json::json!({"kind": "agent_stalled", "doing": "Bash sleep", "quiet_secs": 700})
    );
    let resumed = Event::Stall(StallEvent::AgentResumed { quiet_secs: 900 });
    assert_eq!(resumed.kind(), "agent_resumed");
    assert_eq!(resumed.payload(), serde_json::json!({"quiet_secs": 900}));
}

#[test]
fn node_disk_events_carry_their_own_kind() {
    let low = Event::NodeDisk(NodeDiskEvent::NodeDiskLow {
        node: "hv".to_owned(),
        free_mib: 9_000,
        total_mib: 409_600,
    });
    assert_eq!(low.kind(), "node_disk_low");
    assert_eq!(
        low.payload(),
        serde_json::json!({"node": "hv", "free_mib": 9_000, "total_mib": 409_600})
    );
    let json = serde_json::to_string(&low).unwrap();
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), low);
}

#[test]
fn profile_events_serialize_flat() {
    let resolved = Event::Profile(ProfileEvent::ProfileResolved {
        repo: crate::name::ForgeRepo::new("o", "p"),
        sha: "abc".to_owned(),
        sources: Vec::new(),
        unresolved: Vec::new(),
    });
    assert_eq!(
        serde_json::to_string(&resolved).unwrap(),
        r#"{"kind":"profile_resolved","repo":"o/p","sha":"abc"}"#
    );
    assert_eq!(resolved.kind(), "profile_resolved");
    let failed = Event::Profile(ProfileEvent::ProfileFailed {
        step: ProfileStep::Fetch,
        detail: "d".to_owned(),
    });
    assert_eq!(
        serde_json::to_string(&failed).unwrap(),
        r#"{"kind":"profile_failed","step":"fetch","detail":"d"}"#
    );
    assert_eq!(
        failed.payload(),
        serde_json::json!({"step":"fetch","detail":"d"})
    );
}

/// `ArtifactPublished` carries its artifact kind as `artifact_kind`, since `kind` is the tag.
#[test]
fn artifact_published_kind_and_payload() {
    let e = Event::ArtifactPublished {
        name: s("shot.png"),
        version: 2,
        artifact_kind: s("screenshot"),
        content_type: s("image/png"),
        size: 12,
        sha256: s("ab"),
    };
    assert_eq!(e.kind(), "artifact_published");
    assert_eq!(
        e.payload(),
        serde_json::json!({"name":"shot.png","version":2,"artifact_kind":"screenshot","content_type":"image/png","size":12,"sha256":"ab"})
    );
}

/// `TurnDone` names itself and carries the steer id and each outcome.
#[test]
fn turn_done_kind_and_payload() {
    for (outcome, name) in [
        (TurnOutcome::Idle, "idle"),
        (TurnOutcome::NeedsInput, "needs_input"),
        (TurnOutcome::Exited, "exited"),
    ] {
        let e = Event::TurnDone { steer: 7, outcome };
        assert_eq!(e.kind(), "turn_done");
        assert_eq!(e.payload(), serde_json::json!({"steer":7,"outcome":name}));
    }
}

/// `SessionEnded` names itself and carries the documented payload.
#[test]
fn session_ended_kind_and_payload() {
    let e = Event::SessionEnded {
        session: 3,
        cause: s("exited"),
        exit: None,
    };
    assert_eq!(e.kind(), "session_ended");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({"kind":"session_ended","session":3,"cause":"exited","exit":null})
    );
    assert_eq!(
        e.payload(),
        serde_json::json!({"session":3,"cause":"exited","exit":null})
    );
}

/// A stored `copied` or `exec_ran` row reads back as the event that wrote it.
#[test]
fn access_events_read_back_from_their_json() {
    let copied = serde_json::json!({"kind": "copied", "from": "/a", "to": "/b"});
    assert_eq!(
        serde_json::from_value::<Event>(copied).unwrap(),
        Event::Access(AccessEvent::Copied {
            from: s("/a"),
            to: s("/b"),
        })
    );
    let ran = serde_json::json!({"kind": "exec_ran", "command": "df -h", "exit": null,
        "timed_out": true});
    assert_eq!(
        serde_json::from_value::<Event>(ran).unwrap(),
        Event::Access(AccessEvent::ExecRan {
            command: s("df -h"),
            exit: None,
            timed_out: true,
        })
    );
}
