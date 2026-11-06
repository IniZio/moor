//! The `agent_stalled` and `agent_resumed` events a busy hook that goes quiet writes.

use std::time::Duration;

use moor_core::activity::Activity;
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
    fn hooked(&self, kind: HookKind, age: Duration, doing: Option<&str>) {
        let mut record = rec(kind);
        record.at =
            Ts::from_millis(self.clock.now().as_millis() - i64::try_from(age.as_millis()).unwrap());
        record.doing = doing.map(str::to_owned);
        self.cx.store().unwrap().put_hook(self.id, &record).unwrap();
    }

    fn pass(&self, in_flight: bool) {
        self.pass_with(
            Activity {
                state: AgentState::Working,
                source: Source::Hooks,
            },
            in_flight,
        );
    }

    fn pass_with(&self, activity: Activity, in_flight: bool) {
        let r = Reading::new(Some(activity), PaneLiveness::Live, false, in_flight);
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

#[test]
fn a_busy_hook_quiet_for_eleven_minutes_writes_one_stall() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, Some("Bash: cargo test"));
    rig.pass(false);
    rig.pass(false);
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
    rig.hooked(HookKind::Busy, 9 * MINUTE, None);
    rig.pass(false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn a_box_suspended_mid_turn_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, None);
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
    rig.pass(false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn an_idle_hook_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Idle, 11 * MINUTE, None);
    rig.pass(false);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn a_run_in_flight_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, None);
    rig.pass(true);
    assert!(rig.events("agent_stalled").is_empty());
}

#[test]
fn a_fresh_report_after_a_stall_writes_one_resume() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, None);
    rig.pass(false);
    rig.clock.sleep(MINUTE);
    rig.hooked(HookKind::Busy, Duration::ZERO, None);
    rig.pass(false);
    rig.pass(false);
    assert_eq!(rig.events("agent_resumed").len(), 1);
    assert_eq!(rig.events("agent_stalled").len(), 1);
}

#[test]
fn a_second_stall_after_a_resume_is_written_again() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, None);
    rig.pass(false);
    rig.clock.sleep(MINUTE);
    rig.hooked(HookKind::Busy, Duration::ZERO, None);
    rig.pass(false);
    rig.clock.sleep(11 * MINUTE);
    rig.pass(false);
    assert_eq!(rig.events("agent_stalled").len(), 2);
}

#[test]
fn an_unreadable_hook_is_not_a_stall() {
    let (_dir, rig) = rig();
    rig.hooked(HookKind::Busy, 11 * MINUTE, None);
    let unknown = Activity {
        state: AgentState::Unknown,
        source: Source::Timeout,
    };
    rig.pass_with(unknown, false);
    assert!(rig.events("agent_stalled").is_empty());
}
