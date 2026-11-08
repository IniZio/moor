//! The `agent_stalled` and `agent_resumed` events a working agent whose status stops changing writes.

use std::time::Duration;

use moor_core::activity::{Activity, Life, Status, Term};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::seam::clock::Clock;

use super::*;
use crate::testing::clock::FakeClock;

const MINUTE: Duration = Duration::from_secs(60);

struct Rig {
    cx: Cx,
    id: BoxId,
    clock: Arc<FakeClock>,
}

fn rig() -> (tempfile::TempDir, Rig) {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, _) = cx(dir.path(), &Arc::new(FakeEngine::new()));
    let cx = cx.with_clock(clock.clone());
    let id = working_box(&cx, true);
    (dir, Rig { cx, id, clock })
}

impl Rig {
    /// A settle pass for an agent whose `status` last changed `age` ago.
    fn pass(&self, status: Status, age: Duration, in_flight: bool) {
        let state = match status {
            Status::Working { .. } => AgentState::Working,
            _ => AgentState::Idle,
        };
        let activity = Activity {
            state,
            source: Source::Status,
        };
        self.pass_with(activity, Some((status, age)), in_flight);
    }

    fn pass_with(&self, activity: Activity, term: Option<(Status, Duration)>, in_flight: bool) {
        let mut r = Reading::new(Some(activity), PaneLiveness::Live, false, in_flight);
        r.term = term.map(|(status, age)| Term {
            life: Life::Live,
            status: Some(status),
            status_at: Some(Ts::from_millis(
                self.clock.now().as_millis() - i64::try_from(age.as_millis()).unwrap(),
            )),
            turn: 0,
        });
        settle(
            &self.cx,
            self.cx.store().unwrap(),
            &row(&self.cx, self.id),
            &r,
        )
        .unwrap();
    }

    fn events(&self, kind: &str) -> Vec<serde_json::Value> {
        let rows = self.cx.store().unwrap().events(self.id, 100).unwrap();
        rows.into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.payload)
            .collect()
    }
}

fn working(doing: Option<&str>) -> Status {
    Status::Working {
        doing: doing.map(str::to_owned),
    }
}

#[test]
fn a_working_status_unchanged_for_eleven_minutes_writes_one_stall() {
    let (_dir, rig) = rig();
    let busy = working(Some("Bash: cargo test"));
    rig.pass(busy.clone(), 11 * MINUTE, false);
    rig.pass(busy, 11 * MINUTE, false);
    let stalls = rig.events("agent_stalled");
    assert_eq!(stalls.len(), 1, "once per stall");
    let stall = stalls.first().unwrap();
    assert_eq!(stall.get("quiet_secs"), Some(&serde_json::json!(660)));
    assert_eq!(
        stall.get("doing"),
        Some(&serde_json::json!("Bash: cargo test"))
    );
}

#[test]
fn nine_minutes_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.pass(working(None), 9 * MINUTE, false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn exactly_ten_minutes_is_a_stall() {
    let (_dir, rig) = rig();
    rig.pass(working(None), 10 * MINUTE, false);
    assert_eq!(rig.events("agent_stalled").len(), 1);
}

#[test]
fn a_box_suspended_mid_turn_is_not_a_stall() {
    let (_dir, rig) = rig();
    let suspend = BoxLifecycleInput::Suspend {
        reason: "last-resort suspend".to_owned(),
    };
    rig.cx
        .store()
        .unwrap()
        .apply(
            Decision::new(rig.id)
                .lifecycle(suspend)
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    rig.pass(working(None), 11 * MINUTE, false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn an_idle_status_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.pass(Status::Idle, 11 * MINUTE, false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn a_run_in_flight_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.pass(working(None), 11 * MINUTE, true);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn a_status_that_moves_after_a_stall_writes_one_resume() {
    let (_dir, rig) = rig();
    rig.pass(working(None), 11 * MINUTE, false);
    rig.clock.sleep(MINUTE);
    rig.pass(working(None), Duration::ZERO, false);
    rig.pass(working(None), Duration::ZERO, false);
    assert_eq!(rig.events("agent_resumed").len(), 1);
    assert_eq!(rig.events("agent_stalled").len(), 1);
}

#[test]
fn a_second_stall_after_a_resume_is_written_again() {
    let (_dir, rig) = rig();
    rig.pass(working(None), 11 * MINUTE, false);
    rig.clock.sleep(MINUTE);
    rig.pass(working(None), Duration::ZERO, false);
    rig.clock.sleep(11 * MINUTE);
    rig.pass(working(None), 11 * MINUTE, false);
    assert_eq!(rig.events("agent_stalled").len(), 2);
}

#[test]
fn an_unreadable_agent_is_not_a_stall() {
    let (_dir, rig) = rig();
    let unknown = Activity {
        state: AgentState::Unknown,
        source: Source::Timeout,
    };
    rig.pass_with(unknown, Some((working(None), 11 * MINUTE)), false);
    assert!(rig.events("agent_stalled").is_empty());
}
