use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Acquired;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath};

use crate::ops::common::provision::hold_root;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::store::Store;
use crate::testing::clock::FakeClock;

/// Plans a `BoxRoot` row under "planned-1", holds the root for `got`, and returns the
/// state of the row under `key`.
fn state_after_hold(got: &str, key: &str) -> Option<LeaseState> {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("moor.db"),
        Arc::new(FakeClock::new(Ts::from_millis(1_000))),
    )
    .unwrap();
    let id: BoxId = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("demo").unwrap(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let planned = BoxHandle::new("planned-1");
    let root = store
        .plan_lease(id, LeaseKind::BoxRoot, planned.as_str())
        .unwrap();
    let got = Acquired {
        handle: BoxHandle::new(got),
        workdir: BoxPath::restore("/work").unwrap(),
    };
    hold_root(&store, id, root, &planned, &got).unwrap();
    store
        .leases(id)
        .unwrap()
        .iter()
        .find(|r| r.kind == LeaseKind::BoxRoot && r.key == key)
        .map(|r| r.state)
}

#[test]
fn an_acquired_handle_off_the_plan_is_held_and_the_planned_row_stays_planned() {
    assert_eq!(
        state_after_hold("other-1", "planned-1"),
        Some(LeaseState::Planned)
    );
    assert_eq!(
        state_after_hold("other-1", "other-1"),
        Some(LeaseState::Held)
    );
}

#[test]
fn an_acquired_handle_on_the_plan_holds_the_planned_row() {
    assert_eq!(
        state_after_hold("planned-1", "planned-1"),
        Some(LeaseState::Held)
    );
}
