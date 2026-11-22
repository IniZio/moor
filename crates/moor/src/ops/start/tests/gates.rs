//! A start with gates keeps them with the Box, and `show` reports the Box against them.

use std::collections::BTreeSet;

use moor_core::config::UserConfig;
use moor_core::gate::{Approval, GateDoc, GateNext, GateStatus};
use moor_core::view::ShowView;
use moor_core::workflow::StepId;
use moor_core::{BoxName, ErrorKind, ShowReq, StartReq};

use super::agent_install::install_world;
use super::{repo_req, run, script_repo, world, world_with, World};
use crate::testing::driver::FakeDriver;

fn proof() -> GateDoc {
    GateDoc {
        name: "proof".to_owned(),
        evidence: vec!["proof.md".to_owned()],
        checks: Vec::new(),
        approval: None,
        max_tries: None,
    }
}

fn with_gates(name: &str, gates: Vec<GateDoc>) -> StartReq {
    StartReq {
        prompt: Some("Fix the typo.".to_owned()),
        gates: Some(Box::new(gates)),
        ..repo_req(name)
    }
}

/// A user config whose Boxes get the `moor` MCP server, which has `report_step`.
fn serving() -> UserConfig {
    let served = moor_core::config::parse_user("x", "moor_service:\n  upstream: 127.0.0.1:8444\n");
    UserConfig {
        moor_service: served.unwrap().moor_service,
        ..UserConfig::default()
    }
}

/// A controller whose Boxes can call `report_step`: the MCP server is theirs and a front reports.
fn reporting(w: World) -> World {
    let mut config = w.cx.config().clone();
    config.moor_service = serving().moor_service;
    let cx = w.cx.with_config(config);
    World {
        cx: cx.with_env(|e| e.with_front_step_reports(BTreeSet::from(["gate".to_owned()]))),
        ..w
    }
}

fn show(w: &World, name: Option<&str>) -> ShowView {
    let req = ShowReq {
        target: name.map(|n| BoxName::parse_allow_bare(n).unwrap().into()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
        transcript: false,
    };
    crate::ops::show::run(&w.cx, &req).unwrap()
}

#[test]
fn a_start_with_gates_shows_the_box_with_its_first_gate_open() {
    let w = reporting(world(FakeDriver::new()));
    script_repo(&w, None);
    run(&w.cx, &with_gates("demo", vec![proof()])).unwrap();
    let want = GateStatus::Open {
        gate: StepId::parse("proof").unwrap(),
        try_n: 1,
        next: GateNext::Agent,
    };
    let detail = show(&w, Some("demo")).boxed.unwrap();
    let gate = detail.gate.unwrap();
    assert_eq!(
        (gate.gates, gate.status),
        (vec!["proof".to_owned()], want.clone())
    );
    let listed = show(&w, None).boxes.unwrap();
    assert_eq!(listed.first().unwrap().gate.as_ref().unwrap().status, want);
}

#[test]
fn a_start_without_gates_shows_none() {
    let w = world(FakeDriver::new());
    script_repo(&w, None);
    run(&w.cx, &repo_req("demo")).unwrap();
    assert!(show(&w, Some("demo")).boxed.unwrap().gate.is_none());
}

#[test]
fn gates_that_are_not_valid_refuse_the_start_before_a_box_exists() {
    let w = world(FakeDriver::new());
    script_repo(&w, None);
    let nothing = GateDoc {
        evidence: Vec::new(),
        ..proof()
    };
    let err = run(&w.cx, &with_gates("demo", vec![nothing])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.error.to_string().contains("proof"), "{}", err.error);
    assert!(w.cx.store().unwrap().open_boxes().unwrap().is_empty());
}

#[test]
fn a_start_of_an_open_box_keeps_its_gates_and_refuses_others() {
    let w = reporting(world(FakeDriver::new()));
    script_repo(&w, None);
    run(&w.cx, &with_gates("demo", vec![proof()])).unwrap();
    run(&w.cx, &with_gates("demo", vec![proof()])).unwrap();
    run(&w.cx, &repo_req("demo")).unwrap();
    let other = GateDoc {
        approval: Some(Approval::Owner),
        ..proof()
    };
    let err = run(&w.cx, &with_gates("demo", vec![other])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    let kept = show(&w, Some("demo")).boxed.unwrap().gate.unwrap();
    assert_eq!(kept.gates, ["proof"]);
}

#[test]
fn a_gated_box_first_message_carries_its_gates_and_a_plain_one_stays_bare() {
    let (w, engine) = install_world("", None, false, FakeDriver::new());
    let w = reporting(w);
    let asked = StartReq {
        prompt: Some("Fix the typo.".to_owned()),
        ..with_gates("demo", vec![proof()])
    };
    run(&w.cx, &asked).unwrap();
    let first = engine.launches().remove(0).prompt;
    let text = first.text().unwrap();
    assert!(
        text.starts_with("Fix the typo.\n\nmoor keeps this Box open"),
        "{text}"
    );
    assert!(
        text.contains("Gate `proof`: leave proof.md in .moor/artifacts/"),
        "{text}"
    );

    let (w, engine) = install_world("", None, false, FakeDriver::new());
    let plain = StartReq {
        prompt: Some("Fix the typo.".to_owned()),
        ..repo_req("demo")
    };
    run(&w.cx, &plain).unwrap();
    assert_eq!(
        engine.launches().remove(0).prompt.text(),
        Some("Fix the typo.")
    );
}

fn refused_start(w: &World, req: &StartReq) -> String {
    let err = run(&w.cx, req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage, "{err:?}");
    assert!(w.cx.store().unwrap().open_boxes().unwrap().is_empty());
    err.error.to_string()
}

#[test]
fn evidence_or_approval_on_a_controller_that_cannot_offer_report_step_is_refused() {
    let w = world(FakeDriver::new());
    script_repo(&w, None);
    let approval = GateDoc {
        evidence: Vec::new(),
        approval: Some(Approval::Owner),
        ..proof()
    };
    for gate in [proof(), approval] {
        let why = refused_start(&w, &with_gates("demo", vec![gate]));
        assert!(
            why.contains("report_step") && why.contains("moor_service"),
            "{why}"
        );
    }
    let no_front = world_with(FakeDriver::new(), serving());
    script_repo(&no_front, None);
    let why = refused_start(&no_front, &with_gates("demo", vec![proof()]));
    assert!(why.contains("step_reports"), "{why}");

    let checks = GateDoc {
        evidence: Vec::new(),
        checks: vec!["true".to_owned()],
        ..proof()
    };
    run(&w.cx, &with_gates("demo", vec![checks])).unwrap();
}

#[test]
fn gates_on_a_start_with_no_prompt_and_no_issue_are_refused() {
    let w = reporting(world(FakeDriver::new()));
    script_repo(&w, None);
    let bare = StartReq {
        prompt: None,
        ..with_gates("demo", vec![proof()])
    };
    let why = refused_start(&w, &bare);
    assert!(why.contains("prompt or an issue"), "{why}");
    let issue = StartReq {
        issue: Some("o/n#1".to_owned()),
        ..bare
    };
    run(&w.cx, &issue).unwrap();
}
