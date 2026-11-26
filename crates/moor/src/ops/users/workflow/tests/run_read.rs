//! `workflow-run-read`: one run by its id, for anyone signed in.

use moor_core::request::WorkflowCmd;
use moor_core::workflow::RunState;

use super::{cmd, daily, fixture, me, ok, record, refusal, run_list};

#[test]
fn any_signed_in_person_reads_a_run_by_its_id_and_only_its_owner_is_told_it_is_theirs() {
    let f = fixture();
    let owner = me(&f, "local:2000");
    daily(&f, &owner);
    record(&f, &owner, RunState::Queued, 100);
    let id = run_list(&f, &owner, None)["runs"][0]["id"]
        .as_i64()
        .unwrap();
    let other = me(&f, "local:2001");
    let read = ok(&f, &other, cmd(WorkflowCmd::RunRead { run: id }));
    assert_eq!(read["workflow"], "daily");
    assert_eq!(read["yours"], false);
    assert_eq!(read["run"]["id"], id);
    assert_eq!(read["run"]["due_at"], 100);
    assert_eq!(read["run"]["plan"]["steps"][0]["kind"], "deliver");
    let mine = ok(&f, &owner, cmd(WorkflowCmd::RunRead { run: id }));
    assert_eq!(mine["yours"], true);
    let why = refusal(&f, &other, cmd(WorkflowCmd::RunRead { run: id + 99 }));
    assert!(why.contains("no run"), "{why}");
}
