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
        offer: Some(Offer {
            max_boxes: Some(4),
            cache_gb: Some(20),
            features: vec!["cache-volume".to_owned()],
            mem_mib: Some(31_232),
            cpus: Some(16),
            reserve_mib: None,
        }),
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
