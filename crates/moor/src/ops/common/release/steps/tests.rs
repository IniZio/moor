//! Tests for the release plan, its scripts and the lease rows each step answers for.

use moor_core::seam::clock::Ts;
use moor_core::task_key::{MacLayout, TaskKey};
use moor_core::{BoxPath, TaskId, TaskName};

use super::{plan, rows_for, script, MacTarget, Step};
use crate::store::lease::{LeaseId, LeaseKind, LeaseRow, LeaseState, ProcessIds};

fn row(id: i64, kind: LeaseKind, key: &str, pgid: Option<i64>) -> LeaseRow {
    LeaseRow {
        id: LeaseId(id),
        task: TaskId::new(1),
        kind,
        key: key.to_owned(),
        state: LeaseState::Held,
        booted: false,
        process: pgid.map(|pgid| ProcessIds {
            pgid,
            sid: pgid,
            root_pid: pgid,
            root_start: 0,
        }),
        size_at_release: None,
        updated_at: Ts::from_millis(0),
        attempts: 0,
        failure: None,
        next_attempt_at: None,
    }
}

struct Fixture {
    key: TaskKey,
    layout: MacLayout,
}

impl Fixture {
    fn new() -> Self {
        let key = TaskKey::new(&TaskName::parse("demo").unwrap());
        let layout = MacLayout::new(&BoxPath::restore("/w").unwrap(), &key).unwrap();
        Self { key, layout }
    }

    fn target(&self) -> MacTarget<'_> {
        MacTarget {
            key: &self.key,
            layout: &self.layout,
        }
    }
}

const MAC_NO_COMPOSE: [Step; 7] = [
    Step::StopAgent,
    Step::KillTree,
    Step::OrphanIbtoold,
    Step::DeleteDevices,
    Step::Teardown,
    Step::RemoveRoot,
    Step::Verify,
];

#[test]
fn mac_plan_without_compose_row() {
    let f = Fixture::new();
    let rows = [row(1, LeaseKind::BoxRoot, "/w/demo", None)];
    assert_eq!(plan(Some(&f.target()), &rows), MAC_NO_COMPOSE);
}

#[test]
fn mac_plan_with_compose_row_downs_after_teardown() {
    let f = Fixture::new();
    let rows = [row(1, LeaseKind::Compose, "proj", None)];
    assert_eq!(
        plan(Some(&f.target()), &rows),
        [
            Step::StopAgent,
            Step::KillTree,
            Step::OrphanIbtoold,
            Step::DeleteDevices,
            Step::Teardown,
            Step::ComposeDown,
            Step::RemoveRoot,
            Step::Verify,
        ]
    );
}

#[test]
fn non_mac_plan_is_teardown_then_optional_compose() {
    assert_eq!(plan(None, &[]), [Step::Teardown]);
    let rows = [row(1, LeaseKind::Compose, "proj", None)];
    assert_eq!(plan(None, &rows), [Step::Teardown, Step::ComposeDown]);
}

#[test]
fn kill_tree_script_carries_recorded_pgids() {
    let f = Fixture::new();
    let rows = [
        row(1, LeaseKind::Process, "a", Some(4242)),
        row(2, LeaseKind::Process, "b", None),
        row(3, LeaseKind::Process, "c", Some(5151)),
    ];
    let got = script(Step::KillTree, Some(&f.target()), &rows).unwrap();
    assert!(got.contains("4242") && got.contains("5151"), "{got}");
}

#[test]
fn compose_down_uses_the_row_key() {
    let rows = [row(1, LeaseKind::Compose, "my proj", None)];
    assert_eq!(
        script(Step::ComposeDown, None, &rows).unwrap(),
        "docker compose -p 'my proj' down -v --remove-orphans"
    );
    assert_eq!(script(Step::ComposeDown, None, &[]), None);
}

#[test]
fn teardown_has_no_script() {
    let f = Fixture::new();
    assert_eq!(script(Step::Teardown, Some(&f.target()), &[]), None);
}

#[test]
fn mac_steps_have_scripts_only_with_a_target() {
    let f = Fixture::new();
    for step in MAC_NO_COMPOSE {
        if step == Step::Teardown {
            continue;
        }
        assert!(script(step, Some(&f.target()), &[]).is_some(), "{step:?}");
        assert!(script(step, None, &[]).is_none(), "{step:?}");
    }
}

#[test]
fn rows_for_maps_kinds_to_steps() {
    let rows = [
        row(1, LeaseKind::BoxRoot, "/w/demo", None),
        row(2, LeaseKind::Process, "p", Some(1)),
        row(3, LeaseKind::SimSet, "s", None),
        row(4, LeaseKind::SimDevice, "d", None),
        row(5, LeaseKind::Compose, "c", None),
        row(6, LeaseKind::Cache, "/w/demo/cache", None),
        row(7, LeaseKind::Home, "/w/demo/home", None),
        row(
            8,
            LeaseKind::ToolchainLink,
            "/Users/me/.local/share/mise",
            None,
        ),
    ];
    assert_eq!(rows_for(Step::KillTree, &rows), [LeaseId(2)]);
    assert_eq!(
        rows_for(Step::DeleteDevices, &rows),
        [LeaseId(3), LeaseId(4)]
    );
    assert_eq!(rows_for(Step::ComposeDown, &rows), [LeaseId(5)]);
    assert_eq!(
        rows_for(Step::RemoveRoot, &rows),
        [LeaseId(1), LeaseId(6), LeaseId(7), LeaseId(8)]
    );
}

#[test]
fn rows_for_falls_back_to_box_root_then_first_row() {
    let rows = [
        row(1, LeaseKind::Compose, "c", None),
        row(2, LeaseKind::BoxRoot, "/w/demo", None),
    ];
    assert_eq!(rows_for(Step::Verify, &rows), [LeaseId(2)]);
    assert_eq!(rows_for(Step::KillTree, &rows), [LeaseId(2)]);
    let rows = [row(1, LeaseKind::Compose, "c", None)];
    assert_eq!(rows_for(Step::StopAgent, &rows), [LeaseId(1)]);
    assert_eq!(rows_for(Step::StopAgent, &[]), []);
}

#[test]
fn every_step_has_a_label() {
    for step in MAC_NO_COMPOSE.into_iter().chain([Step::ComposeDown]) {
        assert!(!step.label().is_empty(), "{step:?}");
    }
}
