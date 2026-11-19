//! A run refused because its Box's setup failed says why, as the Box's `setup_failed` event does.

#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::workflow::{RunEnd, RunState};

use super::{rig, DAY, FIRST_DUE};

const WHY: &str = "exit 143\noutput ended:\nmoor setup: $ sleep 77\nTerminated";

#[test]
fn a_refused_run_carries_the_exit_code_and_output_of_the_failed_setup() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let failed = BoxLifecycleInput::WorkingSetupFailed {
        cause: "sleep 77".to_owned(),
    };
    let store = r.cx.store().unwrap();
    let done = Decision::new(box_id)
        .lifecycle(failed)
        .setup_why(Some(WHY.to_owned()));
    store.apply(done).unwrap();
    r.advance_to(FIRST_DUE + DAY + 41_000);
    r.pass();

    let RunState::Done(RunEnd::Refused { detail }) = r.latest(id).state else {
        panic!("not refused");
    };
    assert!(detail.contains("setup failed: sleep 77\n"), "{detail}");
    assert!(detail.contains("exit 143"), "{detail}");
    assert!(detail.contains("Terminated"), "{detail}");
}
