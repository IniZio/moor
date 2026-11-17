//! The hidden `box-term` verb.

use super::{run, Action};

#[test]
fn box_term_takes_a_runner_a_machine_a_slug_a_place_and_one_script() {
    let ok = run(&[
        "box-term",
        "--runner",
        "/r.sock",
        "12",
        "moor-1a2b3c4d-7",
        "--place",
        "work",
        "--",
        "echo hi",
    ]);
    assert!(matches!(
        ok,
        Ok((_, Action::BoxTerm(t)))
            if t.machine_id == 12 && t.place == moor_core::seam::driver::Place::Work && t.script == "echo hi"
    ));
    assert!(run(&[
        "box-term",
        "--runner",
        "/r.sock",
        "12",
        "moor-1a2b3c4d-7",
        "--place",
        "work"
    ])
    .is_err());
    assert!(run(&[
        "box-term",
        "--runner",
        "/r.sock",
        "12",
        "moor-1a2b3c4d-7",
        "--",
        "x"
    ])
    .is_err());
}
