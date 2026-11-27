//! A start a front sends is a plain Box: it runs no agent, and one that carries what only an
//! agent reads is refused before anything changes.

use std::sync::Arc;

use moor_core::{ErrorKind, StartReq};

use super::relaunch::session;
use super::{req, world};
use crate::ops::start::front::from_front;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

#[test]
fn a_front_start_makes_a_box_that_runs_no_agent() {
    let engine = Arc::new(FakeEngine::new().launch_result(Ok(session("p-1"))));
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let v = from_front(&w.cx, req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert!(engine.launches().is_empty());
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_box_by_name(&req("demo").name)
        .unwrap()
        .is_some());
}

#[test]
fn a_front_start_carrying_what_only_an_agent_reads_is_refused_and_makes_no_box() {
    let w = world(FakeDriver::new());
    let carrying = [
        StartReq {
            prompt: Some("fix it".into()),
            ..req("demo")
        },
        StartReq {
            issue: Some("42".into()),
            ..req("demo")
        },
        StartReq {
            model: Some("haiku".into()),
            ..req("demo")
        },
        StartReq {
            resume: true,
            ..req("demo")
        },
        StartReq {
            gates: Some(Box::new(moor_core::gate::GatesDoc {
                gates: Vec::new(),
                outcomes: Vec::new(),
                made_from: None,
            })),
            ..req("demo")
        },
    ];
    for (r, what) in carrying
        .into_iter()
        .zip(["prompt", "issue", "model", "resume", "gates"])
    {
        let e = from_front(&w.cx, r).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Usage, "{}", e.error);
        assert_eq!(
            e.error.to_string(),
            format!("a Box a person starts runs no agent, so its start takes no {what}")
        );
        assert_eq!(
            e.hints,
            ["to start a task with words, type `moor ask --repo OWNER/NAME TEXT` or use the ask \
              tool; its agent starts work Boxes as it needs them"]
        );
    }
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_box_by_name(&req("demo").name)
        .unwrap()
        .is_none());
}
