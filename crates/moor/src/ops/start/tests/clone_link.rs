//! A machine's link that drops while the Box clones its repo: the clone is asked again once the
//! link is back, so a controller deploy does not fail the Box's setup.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::DriverError;

use super::*;

/// What the clone exec runs.
const CLONE: &str = "git clone";

/// What the machine adapter says when the stream it carried a command on ends early.
fn cut() -> DriverError {
    DriverError::Transport {
        stderr_tail: "the stream ended before the command did".to_owned(),
    }
}

fn asked(w: &World, needle: &str) -> usize {
    let execs = w.driver.execs();
    execs
        .iter()
        .filter(|(_, spec, _)| spec.argv.join(" ").contains(needle))
        .count()
}

#[test]
fn a_link_drop_during_the_clone_waits_for_the_relink_and_the_box_starts() {
    let driver = FakeDriver::new()
        .exec_fails_once_when(CLONE, cut())
        .exec_fails_once_when(CLONE, cut());
    let w = world(driver);
    script_repo(&w, None);
    let v = run(&w.cx, &repo_req("demo")).unwrap();
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), state::WORKING);
    assert_eq!(asked(&w, CLONE), 3, "two asks cut, one after");
}

#[test]
fn a_clone_cut_on_every_ask_still_fails_setup_naming_the_clone() {
    let w = world(FakeDriver::new().exec_fails_when(CLONE, cut()));
    script_repo(&w, None);
    let err = run(&w.cx, &repo_req("demo")).unwrap_err();
    let text = err.error.to_string();
    assert!(
        text.starts_with("cloning the repo into the Box failed: moor lost the ssh connection"),
        "{text}"
    );
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::SetupFailed);
}
