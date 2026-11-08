//! A launch after a wake waits until the guest answers.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::driver::DriverError;
use moor_core::BoxPath;

use super::{fx, kinds, row, sleep, wake_and_revive, with_agent, Fx};
use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::BoxAgent;

const STALLED: DriverError = DriverError::Timeout(Duration::from_secs(5));

fn environ() -> BoxPath {
    BoxPath::restore("/proc/self/environ").unwrap()
}

fn asleep_without_agent() -> Fx {
    let mut f = fx();
    with_agent(&f);
    let store = f.cx.store().unwrap();
    store.end_current_session(f.id, "exited", None).unwrap();
    sleep(&f);
    let handle = row(&f).boxed.unwrap().handle.unwrap();
    f.driver
        .set_peek_text(handle.as_str(), &environ(), "HOME=/home/box\0USER=box\0");
    f.cx =
        f.cx.with_engine(Arc::new(BoxAgent::new(f.driver.clone(), Arc::new(Claude))));
    f
}

fn failure_step(f: &Fx) -> Option<String> {
    row(f)
        .launch_failure
        .map(|l| format!("{}: {}", l.step, l.message))
}

fn waited(f: &Fx, from: i64) -> i64 {
    f.cx.clock().now().as_millis() - from
}

#[test]
fn the_launch_waits_for_a_guest_that_answers_late() {
    let f = asleep_without_agent();
    f.driver.silent_peeks(vec![
        STALLED,
        DriverError::Transport {
            stderr_tail: "kex_exchange_identification: Connection reset by peer".to_owned(),
        },
        STALLED,
    ]);
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(failure_step(&f), None);
    assert!(store.live_session(f.id).unwrap().is_some());
    assert!(!kinds(&f).contains(&"agent_unreachable".to_owned()));
}

#[test]
fn a_guest_that_never_answers_is_reported_within_the_deadline() {
    let f = asleep_without_agent();
    f.driver.fail_peek_of(&environ(), STALLED);
    let store = f.cx.store().unwrap();
    let from = f.cx.clock().now().as_millis();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    let took = waited(&f, from);
    assert!((120_000..=125_000).contains(&took), "{took}");
    let unreachable = store
        .events(f.id, 100)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "agent_unreachable")
        .expect("agent_unreachable");
    let error = unreachable
        .payload
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap();
    assert!(error.contains("timed out after 5s"), "{error}");
    assert!(store.live_session(f.id).unwrap().is_none());
    assert!(failure_step(&f).is_some_and(|s| s.starts_with("home: ")));
}

#[test]
fn a_guest_already_up_costs_one_look() {
    let f = asleep_without_agent();
    let store = f.cx.store().unwrap();
    assert!(wake_and_revive(&f.cx, store, &row(&f)).unwrap());
    assert_eq!(failure_step(&f), None);
    let launch_looks = 2;
    let looks = f
        .driver
        .peeks()
        .iter()
        .filter(|(_, p)| *p == environ())
        .count();
    assert_eq!(looks, 1 + launch_looks);
}
