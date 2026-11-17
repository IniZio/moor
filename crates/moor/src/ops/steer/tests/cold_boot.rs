//! A steer queued for a Box whose machine cold-boots on wake and loses the agent process.

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::engine::{SessionRef, Steer};
use moor_core::{BoxHandle, BoxPath};

use super::queue::{pass, reports_idle};
use super::{fx_with, put_to_sleep, run, text, Fx};
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

fn relaunched(id: &str) -> SessionRef {
    SessionRef {
        id: id.into(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
    }
}

/// A sleeping Box with a stored agent session, whose relaunch is session `agent-2`.
fn asleep() -> Fx {
    let engine = FakeEngine::new().launch_result(Ok(relaunched("agent-2")));
    let f = fx_with(engine, true);
    let set = Fields {
        agent_session_id: Field::Set("claude-1".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(set))
        .unwrap();
    put_to_sleep(&f);
    f
}

fn relaunch_events(f: &Fx) -> Vec<serde_json::Value> {
    let rows = f.cx.store().unwrap().events(f.id, 100).unwrap();
    rows.into_iter()
        .filter(|e| e.kind == "relaunched")
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_cold_boot_wake_relaunches_the_agent_and_the_steer_reaches_it() {
    let f = asleep();
    let engine = f.engine.clone();
    f.driver
        .on_wake(move || engine.set_probes("agent-1", agent::gone()));
    run(&f.cx, &text("carry on")).unwrap();
    reports_idle(&f, "agent-2");
    pass(&f);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    let launches = f.engine.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches.first().and_then(|l| l.resume.as_deref()),
        Some("claude-1")
    );
    assert_eq!(
        f.engine.steers(),
        vec![("agent-2".to_owned(), Steer::Text("carry on".into()))]
    );
    let events = relaunch_events(&f);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().and_then(|e| e.get("cause")),
        Some(&"wake".into())
    );
}

#[test]
fn a_surviving_agent_is_steered_in_place_after_wake() {
    let f = asleep();
    run(&f.cx, &text("carry on")).unwrap();
    reports_idle(&f, "agent-1");
    pass(&f);
    assert!(f.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert!(f.engine.launches().is_empty());
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("carry on".into()))]
    );
    assert!(relaunch_events(&f).is_empty());
}

/// Runs `ls` in Box `f`, as `exec_in_box` does.
fn exec_ls(f: &Fx) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let pipes = moor_core::seam::proc::Pipes {
        stdin: Box::new(std::io::empty()),
        stdout: &mut out,
        stderr: &mut err,
        stop: &|| false,
    };
    let req = moor_core::ExecReq {
        target: moor_core::BoxName::parse_allow_bare("t").unwrap().into(),
        argv: vec!["ls".to_owned()],
        timeout_secs: None,
    };
    let view = crate::ops::exec::run(&f.cx, &req, pipes).unwrap();
    assert_eq!(view.exit, Some(0));
}

#[test]
fn an_exec_in_an_asleep_box_leaves_its_agent_down_until_a_steer_wakes_it() {
    let engine = FakeEngine::new()
        .probes("agent-1", agent::at_turn(super::working(), 0))
        .launch_result(Ok(relaunched("agent-2")));
    let f = fx_with(engine, true);
    let set = Fields {
        agent_session_id: Field::Set("claude-1".into()),
        ..Fields::default()
    };
    let store = f.cx.store().unwrap();
    store.apply(Decision::new(f.id).set(set)).unwrap();
    run(&f.cx, &text("old work")).unwrap();
    put_to_sleep(&f);
    let engine = f.engine.clone();
    f.driver
        .on_wake(move || engine.set_probes("agent-1", agent::gone()));

    exec_ls(&f);
    pass(&f);

    let ls = f
        .driver
        .execs()
        .into_iter()
        .filter(|(_, spec, _)| spec.argv.last().map(String::as_str) == Some("ls"))
        .count();
    assert_eq!(ls, 1, "the command ran");
    assert!(f.engine.launches().is_empty(), "no agent was launched");
    assert!(relaunch_events(&f).is_empty());
    assert_eq!(
        f.engine.steers(),
        vec![("agent-1".to_owned(), Steer::Text("old work".into()))],
        "nothing was replayed"
    );
    let row = store.box_row(f.id).unwrap().unwrap();
    assert!(row.asleep(), "the Box is still asleep, its agent down");
    assert!(
        f.driver.calls().iter().any(|c| c.starts_with("sleep ")),
        "the machine went back to sleep"
    );

    run(&f.cx, &text("next")).unwrap();
    reports_idle(&f, "agent-2");
    pass(&f);
    assert_eq!(f.engine.launches().len(), 1, "the steer woke the agent");
    let to_new: Vec<_> = f
        .engine
        .steers()
        .into_iter()
        .filter(|(s, _)| s == "agent-2")
        .map(|(_, s)| s)
        .collect();
    assert!(to_new.contains(&Steer::Text("next".into())), "{to_new:?}");
}
