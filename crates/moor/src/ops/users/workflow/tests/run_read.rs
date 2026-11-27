//! `workflow-run-read`: one run by its id, for anyone signed in.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::ContractEvent;
use moor_core::request::WorkflowCmd;
use moor_core::workflow::RunState;

use super::{cmd, daily, fixture, me, ok, record, refusal, run_list};
use crate::testing::runs::running;

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

#[test]
fn a_run_s_view_names_the_contract_version_its_box_s_first_session_read() {
    let f = fixture();
    let owner = me(&f, "local:2000");
    daily(&f, &owner);
    let store = f.cx.store().unwrap();
    let user = store.user_of(&owner).unwrap().unwrap();
    let name = crate::testing::box_name("daily-run");
    let made = store.apply(crate::testing::cx::new_box(name).requester(user));
    let box_id = made.unwrap().box_id;
    for version in [3, 4] {
        let given = ContractEvent::ContractGiven {
            version,
            sha256: "ab".into(),
            harness: "claude".into(),
        };
        let event = Decision::new(box_id).event(ExtraEvent::Contract(given));
        store.apply(event).unwrap();
    }
    record(&f, &owner, running(box_id.get()), 100);
    record(&f, &owner, RunState::Queued, 200);
    let runs = run_list(&f, &owner, None);
    let id_due = |due: i64| {
        runs["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["due_at"] == due)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let read = ok(&f, &owner, cmd(WorkflowCmd::RunRead { run: id_due(100) }));
    assert_eq!(read["contract"], 3);
    let queued = ok(&f, &owner, cmd(WorkflowCmd::RunRead { run: id_due(200) }));
    assert!(queued.get("contract").is_none(), "{queued}");
}
