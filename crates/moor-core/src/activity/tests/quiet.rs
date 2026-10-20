//! Which readings may apply quiet.

use super::{exited, fresh, obs, Case, BACKGROUND};
use crate::activity::{
    may_apply_quiet, Activity, AgentState as S, HookKind as K, HookRead, PaneRead, PaneStatus as P,
    Source,
};

fn quiet(c: &Case) -> bool {
    may_apply_quiet(&c.got(), Some(&c.probes))
}

#[test]
fn hook_idle_and_blocked_apply() {
    let none = || PaneRead::Absent;
    let t = || HookRead::TimedOut;
    assert!(quiet(&Case::new("fresh idle", fresh(K::Idle), none())));
    assert!(quiet(&Case::new(
        "fresh blocked",
        fresh(K::Blocked),
        none()
    )));
    assert!(quiet(
        &Case::new("cached idle", t(), none()).cache(K::Idle, 5)
    ));
    assert!(quiet(
        &Case::new("cached blocked", t(), none()).cache(K::Blocked, 5)
    ));
}

#[test]
fn other_readings_do_not_apply() {
    let a = || HookRead::Absent;
    assert!(!quiet(&Case::new(
        "busy hook",
        fresh(K::Busy),
        PaneRead::Absent
    )));
    assert!(!quiet(&Case::new("herdr idle", a(), obs(P::Idle, ""))));
    assert!(!quiet(&Case::new(
        "herdr blocked",
        a(),
        obs(P::Blocked, "q")
    )));
    assert!(!quiet(&Case::new(
        "dropped read",
        HookRead::TimedOut,
        obs(P::Idle, "")
    )));
    assert!(!quiet(
        &Case::new("starting", a(), obs(P::Idle, "")).started(5)
    ));
    assert!(!quiet(&Case::new(
        "background override",
        fresh(K::Idle),
        obs(P::Idle, BACKGROUND)
    )));
}

#[test]
fn ended_or_missing_session_applies() {
    let a = || HookRead::Absent;
    assert!(quiet(&Case::new("exited pane", a(), exited(1))));
    assert!(quiet(&Case::new(
        "gone pane",
        fresh(K::Busy),
        PaneRead::Gone
    )));
    let unknown = Activity {
        state: S::Unknown,
        source: Source::Pane,
    };
    assert!(may_apply_quiet(&unknown, None), "no live session");
}
