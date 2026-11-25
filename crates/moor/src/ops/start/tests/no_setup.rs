//! A Box started with no setup runs none of its repos' setup lines, on any machine it gets.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::Driver as _;
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxId, StartReq};

use super::setup::{argvs, is_setup_line, start_req};
use super::{repo_req, run, script_repo, world, World, BASE};
use crate::testing::cx::store_config;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

/// A start of `demo` that skips setup.
fn no_setup() -> StartReq {
    StartReq {
        no_setup: true,
        ..start_req()
    }
}

/// The setup lines that ran in any Box of the world.
fn setup_lines(w: &World) -> Vec<Vec<String>> {
    argvs(w).into_iter().filter(|a| is_setup_line(a)).collect()
}

/// The `box_progress` texts of Box `id`, oldest first.
fn progress(w: &World, id: BoxId) -> Vec<String> {
    let mut events = w.cx.store().unwrap().events(id, 100).unwrap();
    events.reverse();
    events
        .into_iter()
        .filter(|e| e.kind == "box_progress")
        .filter_map(|e| Some(e.payload.get("text")?.as_str()?.to_owned()))
        .collect()
}

const SKIPPED: &str = "setup skipped: started with no setup";

#[test]
fn a_start_with_no_setup_runs_no_setup_line_and_says_it_skipped_them() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("setup: [a, b]\n"));
    let v = run(&w.cx, &no_setup()).unwrap();
    let id = BoxId::new(v.boxed.id);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
    assert!(!v.setup.ran, "{:?}", v.setup);
    assert_eq!(setup_lines(&w), Vec::<Vec<String>>::new());
    assert_eq!(progress(&w, id), [SKIPPED]);
    assert!(w.cx.store().unwrap().box_row(id).unwrap().unwrap().no_setup);
}

#[test]
fn a_start_without_the_flag_still_runs_the_setup_lines() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("setup: [a]\n"));
    let v = run(&w.cx, &start_req()).unwrap();
    assert!(v.setup.ran);
    assert_eq!(setup_lines(&w).len(), 1);
    assert!(progress(&w, BoxId::new(v.boxed.id)).is_empty());
}

#[test]
fn a_start_with_no_setup_skips_the_other_repos_setup_too() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("setup: [main]\n"));
    let lib = ForgeRepo::new("o", "lib");
    store_config(w.cx.store().unwrap(), &lib, "setup: [lib]\n");
    w.forge.answer(lib.clone(), Ok(repo_at(BASE)));
    let req = StartReq {
        no_setup: true,
        also: Some(vec![lib]),
        ..repo_req("demo")
    };
    let v = run(&w.cx, &req).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Ready);
    assert_eq!(setup_lines(&w), Vec::<Vec<String>>::new());
}

#[test]
fn an_asleep_box_with_no_setup_that_lost_its_machine_wakes_on_a_new_one_without_setup() {
    let w = world(FakeDriver::new());
    script_repo(&w, Some("setup: [a]\n"));
    let id = BoxId::new(run(&w.cx, &no_setup()).unwrap().boxed.id);
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
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
    w.driver.release(&BoxHandle::new("box-1")).unwrap();
    run(&w.cx, &start_req()).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        row.hands.and_then(|h| h.handle),
        Some(BoxHandle::new("box-2"))
    );
    assert_eq!(setup_lines(&w), Vec::<Vec<String>>::new());
    assert_eq!(progress(&w, id), [SKIPPED, SKIPPED]);
}
