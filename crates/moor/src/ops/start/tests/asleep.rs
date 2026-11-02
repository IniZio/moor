use std::sync::Arc;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::{EngineError, LaunchPrompt};
use moor_core::{BoxId, StartReq};

use super::relaunch::session;
use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;

fn prompt_req(prompt: &str) -> StartReq {
    StartReq {
        prompt: Some(prompt.into()),
        ..req("demo")
    }
}

/// A Box whose agent failed to launch, then slept on its own.
fn asleep_after_failed_launch() -> (World, Arc<FakeEngine>, BoxId) {
    let failed = EngineError::Failed {
        step: "proxy",
        message: "the tunnel did not come up".to_owned(),
    };
    let engine = Arc::new(
        FakeEngine::new()
            .launch_result(Err(failed))
            .launch_result(Ok(session("p-2"))),
    );
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    let first = run(&w.cx, &prompt_req("first")).unwrap();
    assert!(!first.agent.launched);
    let id = BoxId::new(first.boxed.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Sleep {
                    auto: true,
                    reason: None,
                })
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    (w, engine, id)
}

#[test]
fn starting_an_asleep_box_whose_agent_failed_wakes_it_and_delivers_the_prompt() {
    let (w, engine, id) = asleep_after_failed_launch();
    let v = run(&w.cx, &prompt_req("carry on")).unwrap();
    assert!(v.attached);
    assert!(!v.queued);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("wake ")));
    assert!(v.agent.launched, "the agent launched: {:?}", v.agent);
    let prompts: Vec<_> = engine.launches().into_iter().map(|l| l.prompt).collect();
    assert_eq!(prompts.last(), Some(&LaunchPrompt::Text("carry on".into())));
    assert_eq!(prompts.len(), 2);
    assert!(w.cx.store().unwrap().live_session(id).unwrap().is_some());
}
