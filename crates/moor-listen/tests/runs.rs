//! `moor-listen --workflow` against the real gate and a fake controller: a line each time a run
//! of the workflow starts, enters a step, waits on a person, and ends.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::print_stderr,
    clippy::option_if_let_else,
    clippy::significant_drop_tightening,
    clippy::needless_pass_by_value,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

mod support;

use std::time::Duration;

use serde_json::{json, Value};
use support::world;

const SHA: &str = "abc1234def5678abc1234def5678abc1234def56";

fn change(id: i64, phase: &str, outcome: &str) -> Value {
    json!({"kind": "run", "workflow": "deps", "id": id, "phase": phase, "outcome": outcome,
        "trigger": "manual", "due_at": 1, "history": []})
}

#[test]
fn a_run_prints_its_start_each_step_its_wait_and_its_end_and_history_stays_quiet() {
    let world = world(&["deps-box"]);
    world.moved_before(change(1, "done", "finished"));
    let listen = world.listen_with(&["--workflow", "deps"]);
    world.wait_live(1);
    let pull = |mut c: Value| {
        c["pull"] = json!("o/n#4");
        c["head_sha"] = json!(SHA);
        c
    };
    world.move_run("deps-box", pull(change(2, "queued", "starting")));
    assert_eq!(
        listen.line(),
        "moor: run deps 2 started: manual (o/n#4@abc1234)"
    );
    let mut work = pull(change(2, "running", "started"));
    work["step"] = json!("work");
    work["step_title"] = json!("Fix the alerts");
    world.move_run("deps-box", work);
    assert_eq!(
        listen.line(),
        "moor: run deps 2 step: Fix the alerts (o/n#4@abc1234)"
    );
    let mut ask = pull(change(2, "waiting", "waiting"));
    ask["step"] = json!("ask");
    ask["step_title"] = json!("Ask to ship");
    world.move_run("deps-box", ask);
    assert_eq!(
        listen.line(),
        "moor: run deps 2 waiting: Ask to ship (o/n#4@abc1234)"
    );
    let mut done = pull(change(2, "done", "finished"));
    done["result"] = json!("Opened o/n#5\nwith 3 fixes");
    world.move_run("deps-box", done);
    assert_eq!(
        listen.line(),
        "moor: run deps 2 ended:finished: Opened o/n#5 (o/n#4@abc1234)"
    );
}

#[test]
fn a_run_first_read_part_way_still_says_it_started_and_one_read_done_says_only_its_end() {
    let world = world(&["deps-box"]);
    let listen = world.listen_with(&["--workflow", "deps"]);
    world.wait_live(1);
    let mut work = change(3, "running", "started");
    work["step"] = json!("work");
    world.move_run("deps-box", work);
    assert_eq!(listen.line(), "moor: run deps 3 started: manual");
    assert_eq!(listen.line(), "moor: run deps 3 step: work");
    let mut refused = change(4, "done", "refused");
    refused["detail"] = json!("no node has room");
    world.move_run("deps-box", refused);
    assert_eq!(
        listen.line(),
        "moor: run deps 4 ended:refused: no node has room"
    );
}

#[test]
fn another_workflows_runs_print_nothing() {
    let world = world(&["deps-box"]);
    let listen = world.listen_with(&["--workflow", "deps"]);
    world.wait_live(1);
    let mut other = change(5, "queued", "starting");
    other["workflow"] = json!("other");
    world.move_run("deps-box", other);
    world.move_run("deps-box", change(6, "queued", "starting"));
    assert_eq!(listen.line(), "moor: run deps 6 started: manual");
}

#[test]
fn watching_a_workflow_keeps_listening_after_the_named_box_closes() {
    let world = world(&["repo/a"]);
    let listen = world.listen_with(&["--box", "repo/a", "--workflow", "deps"]);
    world.wait_live(1);
    world.happen("repo/a", 1, "closed", "");
    assert_eq!(listen.line(), "moor: box repo/a closed");
    std::thread::sleep(Duration::from_millis(300));
    world.move_run("repo/a", change(7, "queued", "starting"));
    assert_eq!(listen.line(), "moor: run deps 7 started: manual");
}
