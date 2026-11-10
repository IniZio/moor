//! What an event leaves on a Box's live launch and row is written with it, so a decision reads
//! the launch and not the history.

use std::time::Duration;

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::{Clock as _, Ts};

use super::hands_fence::ready_box;
use super::open;
use crate::store::OwedNote;

#[test]
fn a_relaunch_names_its_cause_on_its_launch_and_clears_the_cap_record() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    let capped = ExtraEvent::RelaunchCapped {
        count: 3,
        window_secs: 600,
    };
    store.apply(Decision::new(id).event(capped)).unwrap();
    assert!(store.relaunch_capped_at(id).unwrap().is_some());
    let start = clock.now();
    clock.sleep(Duration::from_secs(1));
    let launch = store.begin_launch(id, "s").unwrap();
    let relaunched = ExtraEvent::Relaunched {
        prompt: None,
        cause: Some("wake".to_owned()),
    };
    store.apply(Decision::new(id).event(relaunched)).unwrap();
    let read = store.launch(launch).unwrap().unwrap();
    assert_eq!(read.cause.as_deref(), Some("wake"));
    assert_eq!(read.hands_inc, Some(1));
    assert_eq!(store.relaunch_capped_at(id).unwrap(), None);
    assert_eq!(store.caused_launches_since(id, start).unwrap(), 1);
    assert_eq!(store.caused_launches_since(id, clock.now()).unwrap(), 0);
}

#[test]
fn an_unreachable_stretch_keeps_its_first_failure_for_its_launch_only() {
    let dir = tempfile::tempdir().unwrap();
    let (store, clock) = open(dir.path());
    let id = ready_box(&store);
    let launch = store.begin_launch(id, "s").unwrap();
    let unreachable = || ExtraEvent::AgentUnreachable {
        attempts: 4,
        error: "no answer".to_owned(),
    };
    let first = clock.now();
    store.apply(Decision::new(id).event(unreachable())).unwrap();
    clock.sleep(Duration::from_secs(30));
    store.apply(Decision::new(id).event(unreachable())).unwrap();
    let since = |s: &crate::store::Store| s.launch(launch).unwrap().unwrap().unreachable_since;
    assert_eq!(since(&store), Some(first));
    let next = store.begin_launch(id, "s").unwrap();
    let fresh = store.launch(next).unwrap().unwrap().unreachable_since;
    assert_eq!(fresh, None::<Ts>);
}

#[test]
fn a_lost_disk_is_owed_to_the_agent_until_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _clock) = open(dir.path());
    let id = ready_box(&store);
    store
        .apply(Decision::new(id).event(ExtraEvent::DiskLost))
        .unwrap();
    assert_eq!(store.owed_note(id).unwrap(), Some(OwedNote::DiskLost));
    store.set_resume_first(id, None).unwrap();
    assert_eq!(
        store.owed_note(id).unwrap(),
        Some(OwedNote::DiskLost),
        "kept"
    );
    store.drop_owed_note(id).unwrap();
    assert_eq!(store.owed_note(id).unwrap(), None);
}
