#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use serde_json::json;

use super::{Frame, Offer};

#[test]
fn a_hello_without_an_offer_reads_as_none() {
    let hello = json!({"hello": {"machine_id": 7, "version": "0.1.0"}});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    let Frame::Hello { offer, .. } = frame else {
        panic!("not a hello: {frame:?}");
    };
    assert_eq!(offer, None);
}

#[test]
fn a_hello_with_an_offer_round_trips() {
    let frame = Frame::Hello {
        machine_id: 7,
        version: "0.1.0".to_owned(),
        offer: Some(Box::new(Offer {
            max_boxes: Some(4),
            cache_gb: Some(20),
            features: vec!["cache-volume".to_owned()],
            mem_mib: Some(31_232),
            cpus: Some(16),
            reserve_mib: None,
            boxes: None,
        })),
    };
    let text = serde_json::to_string(&frame).unwrap();
    assert!(text.contains("\"max_boxes\":4"), "{text}");
    assert!(text.contains("\"mem_mib\":31232"), "{text}");
    let back: Frame = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
    let Frame::Hello { offer, .. } = back else {
        panic!("not a hello");
    };
    assert_eq!(offer.unwrap().features, ["cache-volume"]);
}

#[test]
fn an_empty_offer_leaves_out_every_field() {
    assert_eq!(serde_json::to_string(&Offer::default()).unwrap(), "{}");
}

#[test]
fn a_report_with_a_field_this_build_does_not_know_still_reads() {
    let opened = json!({"report": {
        "mem_available_mib": 1024,
        "mem_total_mib": 4096,
        "swap_used_mib": 12,
    }});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = opened else {
        panic!("not a report: {opened:?}");
    };
    assert_eq!(report.mem_total_mib, 4096);
}

#[test]
fn a_hello_with_a_field_this_build_does_not_know_still_reads() {
    let hello = json!({"hello": {
        "machine_id": 7, "protocol": 1, "version": "9.9.9", "boot_id": "abc",
    }});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    assert!(matches!(frame, Frame::Hello { machine_id: 7, .. }));
}

#[test]
fn an_offer_from_an_agent_that_still_says_checkpoints_reads_and_drops_them() {
    let hello = json!({"hello": {"machine_id": 7, "version": "0.9.0", "offer": {
        "max_boxes": 4,
        "features": ["workspace-checkpoint", "workspace-fork"],
        "checkpoints": true,
        "checkpoints_off": "crabbox doctor failed: x",
    }}});
    let frame: Frame = serde_json::from_value(hello).unwrap();
    let Frame::Hello { offer, .. } = frame else {
        panic!("not a hello: {frame:?}");
    };
    let offer = offer.unwrap();
    assert_eq!(offer.max_boxes, Some(4));
    assert_eq!(
        serde_json::to_value(&offer).unwrap().get("checkpoints"),
        None
    );
}

#[test]
fn a_lease_verb_from_a_controller_that_still_saves_caches_at_pause_reads() {
    let verb: super::lease::Verb = serde_json::from_value(json!("pause-publish")).unwrap();
    assert_eq!(verb, super::lease::Verb::PausePublish);
}

#[test]
fn a_report_without_the_per_box_list_reads_as_empty_and_writes_none() {
    let opened = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2}});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = &opened else {
        panic!("not a report: {opened:?}");
    };
    assert!(report.boxes.is_empty());
    assert!(!serde_json::to_string(&opened).unwrap().contains("boxes"));
}

#[test]
fn a_report_with_the_per_box_list_round_trips() {
    let opened = super::Opened::Report {
        report: super::Report {
            mem_available_mib: 1,
            mem_total_mib: 2,
            psi_some_avg10: None,
            psi_full_avg10: None,
            disk_free_mib: None,
            disk_total_mib: None,
            boxes: vec![super::BoxMemory {
                slug: "moor-0badcafe-7".to_owned(),
                host_memory_mib: 1536,
                resume_memory_mib: None,
            }],
            box_health: Vec::new(),
        },
    };
    let text = serde_json::to_string(&opened).unwrap();
    assert!(text.contains("\"host_memory_mib\":1536"), "{text}");
    assert_eq!(
        serde_json::from_str::<super::Opened>(&text).unwrap(),
        opened
    );
}

#[test]
fn a_report_from_an_older_agent_has_no_resume_memory() {
    let back: super::BoxMemory =
        serde_json::from_str(r#"{"slug":"moor-0a1b2c3d-1","host_memory_mib":0}"#).unwrap();
    assert_eq!(back.resume_memory_mib, None);
    let with = super::BoxMemory {
        resume_memory_mib: Some(6144),
        ..back
    };
    let text = serde_json::to_string(&with).unwrap();
    assert!(text.contains("\"resume_memory_mib\":6144"), "{text}");
}

#[test]
fn an_offer_and_a_report_that_say_nothing_of_boxes_are_what_they_were() {
    let offer = Offer {
        max_boxes: Some(2),
        ..Offer::default()
    };
    assert_eq!(serde_json::to_string(&offer).unwrap(), r#"{"max_boxes":2}"#);
    let report = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2}});
    let opened: super::Opened = serde_json::from_value(report).unwrap();
    let text = serde_json::to_string(&opened).unwrap();
    assert!(!text.contains("box_health"), "{text}");
}

#[test]
fn an_offer_that_provides_boxes_carries_its_protocol_and_an_old_runner_reads_past_it() {
    let offer = Offer {
        boxes: Some(super::boxes::BoxOffer {
            protocol: super::boxes::PROTOCOL,
            provides: "docker".to_owned(),
        }),
        ..Offer::default()
    };
    let text = serde_json::to_string(&offer).unwrap();
    assert_eq!(text, r#"{"boxes":{"protocol":1,"provides":"docker"}}"#);
    assert_eq!(serde_json::from_str::<Offer>(&text).unwrap(), offer);
}

#[test]
fn box_health_is_carried_raw_whatever_a_later_agent_puts_in_it() {
    let opened = json!({"report": {"mem_available_mib": 1, "mem_total_mib": 2, "box_health": [
        {"slug": "moor-0badcafe-7", "state": "from-the-future", "extra": [1, 2]},
    ]}});
    let opened: super::Opened = serde_json::from_value(opened).unwrap();
    let super::Opened::Report { report } = &opened else {
        panic!("not a report: {opened:?}");
    };
    assert_eq!(report.box_health.len(), 1);
    let text = serde_json::to_string(&opened).unwrap();
    assert!(text.contains("from-the-future"), "{text}");
}

#[test]
fn the_broker_splices_with_a_unit_ask_and_a_tagged_answer() {
    use super::broker::{Ask, Brokered, PoolOp};
    let ask = Ask {
        machine_id: 12,
        op: PoolOp::Boxes,
    };
    assert_eq!(
        serde_json::to_string(&ask).unwrap(),
        r#"{"machine_id":12,"op":{"op":"boxes"}}"#
    );
    assert_eq!(
        serde_json::to_string(&Brokered::Spliced).unwrap(),
        r#"{"broker":"spliced"}"#
    );
    assert_eq!(super::ops::Op::from(PoolOp::Boxes), super::ops::Op::Boxes);
}
