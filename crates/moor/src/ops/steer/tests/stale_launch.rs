//! The inbox pass reads the agent before it takes the Box's turn. When another op relaunched the
//! agent in between, the pass acts on the launch it read, which is no longer live, so it never
//! ends or replaces the new launch.

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::inbox::InboxKind;
use moor_core::seam::clock::Ts;

use super::queue::pass;
use super::{fx, Fx};
use crate::store::LaunchEnd;
use crate::testing::agent;

/// After the pass's first read, ends the live launch as a relaunch by another op does and begins
/// the next one, whose terminal reads as `next`.
fn relaunch_after_the_read(f: &Fx, next: moor_core::activity::Probe) {
    let store = f.cx.store().unwrap().clone();
    let engine = f.engine.clone();
    let id = f.id;
    f.engine.on_probe(move || {
        let old = store.live_launch(id).unwrap().unwrap();
        let end = LaunchEnd {
            session: Some("relaunch"),
            ..LaunchEnd::default()
        };
        store.end_launch(id, old.id, &end).unwrap();
        let session = store.open_session(id).unwrap();
        store.start_session(session.id).unwrap();
        store.begin_launch(id, "agent-1").unwrap();
        engine.set_probes("agent-1", next);
    });
}

/// Box `t` whose agent session is stored, so the pass may relaunch its agent.
fn resumable() -> Fx {
    let f = fx();
    store_agent_session(&f);
    f
}

/// Stores an agent session on the current Session of Box `t`.
fn store_agent_session(f: &Fx) {
    let set = Fields {
        agent_session_id: Field::Set("stored-1".into()),
        ..Fields::default()
    };
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).set(set))
        .unwrap();
}

fn queue(f: &Fx, t: &str) {
    let store = f.cx.store().unwrap();
    store.enqueue(f.id, InboxKind::Steer, t, "steer").unwrap();
}

#[test]
fn a_pass_that_read_launch_n_exited_leaves_a_starting_n_plus_one_alone() {
    let f = resumable();
    f.engine.set_probes("agent-1", agent::exited(Some(1)));
    queue(&f, "hi");
    let launches = f.engine.launches().len();
    relaunch_after_the_read(&f, agent::gone());
    let new = || {
        f.cx.store()
            .unwrap()
            .live_launch(f.id)
            .unwrap()
            .map(|l| l.id)
    };

    pass(&f);

    assert!(new().is_some());
    assert_eq!(
        f.engine.launches().len(),
        launches,
        "the pass launched nothing"
    );
    assert!(f.engine.steers().is_empty());
    assert_eq!(f.cx.store().unwrap().unsent(f.id).unwrap().len(), 1);
}

#[test]
fn a_pass_that_read_launch_n_idle_sends_only_to_n_plus_one() {
    let f = fx();
    f.engine
        .set_probes("agent-1", agent::idle(Ts::from_millis(i64::MAX / 2)));
    queue(&f, "hi");
    relaunch_after_the_read(&f, agent::idle(Ts::from_millis(i64::MAX / 2)));

    pass(&f);

    let store = f.cx.store().unwrap();
    let live = store.live_launch(f.id).unwrap().unwrap().id;
    let sent: Vec<_> = store
        .sent(f.id)
        .unwrap()
        .iter()
        .map(|r| r.delivery.launch())
        .collect();
    assert_eq!(sent, [Some(live)]);
    assert_eq!(f.engine.steers().len(), 1);
}

#[test]
fn a_pass_that_read_launch_n_exited_relaunches_when_n_ended_with_no_successor() {
    let f = resumable();
    f.engine.set_probes("agent-1", agent::exited(Some(1)));
    queue(&f, "hi");
    let launches = f.engine.launches().len();
    let store = f.cx.store().unwrap().clone();
    let id = f.id;
    f.engine.on_probe(move || {
        let old = store.live_launch(id).unwrap().unwrap();
        let end = LaunchEnd {
            exit: Some(1),
            session: Some("exited"),
            ..LaunchEnd::default()
        };
        store.end_launch(id, old.id, &end).unwrap();
    });

    pass(&f);

    assert_eq!(
        f.engine.launches().len(),
        launches + 1,
        "the pass relaunched"
    );
    assert!(f.cx.store().unwrap().live_launch(f.id).unwrap().is_some());
}

#[test]
fn a_steer_relaunch_of_a_launch_another_op_replaced_launches_nothing() {
    let f = resumable();
    let store = f.cx.store().unwrap();
    let old = store.live_launch(f.id).unwrap().unwrap();
    let end = LaunchEnd {
        session: Some("relaunch"),
        ..LaunchEnd::default()
    };
    store.end_launch(f.id, old.id, &end).unwrap();
    let session = store.open_session(f.id).unwrap();
    store.start_session(session.id).unwrap();
    let new = store.begin_launch(f.id, "agent-1").unwrap();
    store_agent_session(&f);
    f.engine.set_probes("agent-1", agent::gone());
    let launches = f.engine.launches().len();

    let got = crate::ops::start::revive_for_steer(&f.cx, store, f.id, Some(&old)).unwrap();

    assert!(got.is_none(), "{got:?}");
    assert_eq!(f.engine.launches().len(), launches);
    assert_eq!(store.live_launch(f.id).unwrap().map(|l| l.id), Some(new));
}

#[test]
fn watch_records_no_launch_failure_for_a_launch_replaced_while_it_polled() {
    let f = resumable();
    f.engine.set_probes("agent-1", agent::exited(Some(1)));
    relaunch_after_the_read(&f, agent::idle(Ts::from_millis(i64::MAX / 2)));
    let launched = moor_core::view::AgentLaunchView {
        launched: true,
        ..moor_core::view::AgentLaunchView::default()
    };

    crate::ops::start::watch_launch(&f.cx, f.cx.store().unwrap(), f.id, launched);

    let store = f.cx.store().unwrap();
    assert!(store
        .box_row(f.id)
        .unwrap()
        .unwrap()
        .launch_failure
        .is_none());
    assert!(store.live_launch(f.id).unwrap().is_some());
}
