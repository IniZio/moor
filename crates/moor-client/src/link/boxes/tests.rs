#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use moor_core::box_key::ControllerId;
use moor_core::seam::driver::Place;
use serde_json::json;

use super::{BoxAnswer, BoxOffer, BoxOp, BoxReport, ReportState, Size, PROTOCOL};
use crate::link::ops::Op;

/// `op` as the line the controller writes.
fn line(op: &BoxOp) -> String {
    serde_json::to_string(op).unwrap()
}

#[test]
fn a_verb_is_a_kebab_case_tag_and_leaves_out_what_is_unset() {
    let open = BoxOp::Open {
        slug: "moor-1a2b3c4d-417".to_owned(),
        class: None,
        cache: None,
    };
    assert_eq!(line(&open), r#"{"verb":"open","slug":"moor-1a2b3c4d-417"}"#);
    let exec = BoxOp::Exec {
        slug: "moor-1a2b3c4d-417".to_owned(),
        place: Place::Work,
        argv: vec!["npm".to_owned(), "ci".to_owned()],
        env: Vec::new(),
        cwd: None,
        stdin: false,
        tty: Some(Size { cols: 80, rows: 24 }),
    };
    assert_eq!(
        line(&exec),
        r#"{"verb":"exec","slug":"moor-1a2b3c4d-417","place":"work","argv":["npm","ci"],"tty":{"cols":80,"rows":24}}"#
    );
}

#[test]
fn every_verb_reads_back_as_written() {
    let slug = || "moor-1a2b3c4d-417".to_owned();
    let ops = [
        BoxOp::Open {
            slug: slug(),
            class: Some("medium".to_owned()),
            cache: None,
        },
        BoxOp::Wake { slug: slug() },
        BoxOp::Sleep { slug: slug() },
        BoxOp::Close { slug: slug() },
        BoxOp::Seal { slug: slug() },
        BoxOp::Env {
            slug: slug(),
            vars: vec![("A".to_owned(), "b".to_owned())],
            ca_pem: Some("pem".to_owned()),
        },
        BoxOp::List {
            controller: ControllerId::parse("1a2b3c4d").unwrap(),
        },
        BoxOp::Exec {
            slug: slug(),
            place: Place::Agent,
            argv: vec!["ls".to_owned()],
            env: vec![("K".to_owned(), "v".to_owned())],
            cwd: Some("/work".to_owned()),
            stdin: true,
            tty: None,
        },
        BoxOp::Forward {
            slug: slug(),
            port: 3000,
            probe: true,
        },
    ];
    for op in ops {
        let back: BoxOp = serde_json::from_str(&line(&op)).unwrap();
        assert_eq!(back, op);
    }
}

#[test]
fn a_request_with_a_field_or_verb_this_build_does_not_know_is_refused() {
    let extra = json!({"verb": "sleep", "slug": "moor-1a2b3c4d-417", "later": 1});
    assert!(serde_json::from_value::<BoxOp>(extra).is_err());
    let verb = json!({"verb": "teleport", "slug": "moor-1a2b3c4d-417"});
    assert!(serde_json::from_value::<BoxOp>(verb).is_err());
}

#[test]
fn an_op_is_not_a_box_op() {
    let exec = r#"{"op":"exec","box_id":7,"env":[],"script":"id","timeout_secs":5}"#;
    assert!(serde_json::from_str::<BoxOp>(exec).is_err());
    assert!(serde_json::from_str::<BoxOp>(r#"{"op":"boxes"}"#).is_err());
}

#[test]
fn the_runners_first_line_is_a_unit_op() {
    assert_eq!(
        serde_json::to_string(&Op::Boxes).unwrap(),
        r#"{"op":"boxes"}"#
    );
    let back: Op = serde_json::from_str(r#"{"op":"boxes"}"#).unwrap();
    assert_eq!(back, Op::Boxes);
}

#[test]
fn answers_are_kebab_case_tagged_lines_and_ignore_what_a_later_agent_adds() {
    let opened = BoxAnswer::Opened {
        workdir: "/home/agent/moor/417".to_owned(),
        generation: 1,
    };
    assert_eq!(
        serde_json::to_string(&opened).unwrap(),
        r#"{"answer":"opened","workdir":"/home/agent/moor/417","generation":1}"#
    );
    let later = json!({"answer": "woke", "generation": 2, "after": "a while"});
    let woke: BoxAnswer = serde_json::from_value(later).unwrap();
    assert_eq!(woke, BoxAnswer::Woke { generation: 2 });
    assert_eq!(
        serde_json::to_string(&BoxAnswer::Done).unwrap(),
        r#"{"answer":"done"}"#
    );
}

#[test]
fn a_report_reads_without_its_optional_fields() {
    let text = r#"{"slug":"moor-1a2b3c4d-417","state":{"broken":{"why":"3 kills"}},"generation":4,"oom_kills":1}"#;
    let report: BoxReport = serde_json::from_str(text).unwrap();
    assert_eq!(
        report.state,
        ReportState::Broken {
            why: "3 kills".to_owned()
        }
    );
    assert_eq!((report.last_mend, report.mem_mib), (None, None));
}

#[test]
fn the_offer_names_the_protocol_version() {
    let offer = BoxOffer {
        protocol: PROTOCOL,
        provides: "docker".to_owned(),
    };
    assert_eq!(
        serde_json::to_string(&offer).unwrap(),
        r#"{"protocol":1,"provides":"docker"}"#
    );
}
