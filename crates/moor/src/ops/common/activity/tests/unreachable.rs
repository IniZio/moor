//! A Box whose agent stops answering reads: marked unreachable, restored when it answers again,
//! and never ended for the silence alone.

use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Clock as _;
use moor_core::BoxId;

use super::turn::steer;
use super::{read, row, settle, working_box, NOW, PROBE_DEADLINE, SESSION};
use crate::cx::Cx;
use crate::testing::agent::working;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;

const WATCHED: &[&str] = &[
    "agent_unreachable",
    "agent_link_restored",
    "session_ended",
    "turn_done",
];

struct Rig {
    _dir: tempfile::TempDir,
    cx: Cx,
    id: BoxId,
    clock: Arc<FakeClock>,
}

fn rig(engine: FakeEngine) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(NOW));
    let cx = Cx::fake_with_clock(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
        moor_core::config::UserConfig::default(),
        clock.clone(),
    )
    .with_engine(Arc::new(engine));
    let id = working_box(&cx, true);
    Rig {
        _dir: dir,
        cx,
        id,
        clock,
    }
}

impl Rig {
    /// One settle pass `secs` after the last.
    fn pass_after(&self, secs: u64) {
        self.clock.sleep(Duration::from_secs(secs));
        let store = self.cx.store().unwrap();
        let now = self.clock.now();
        let reading = read(
            &self.cx,
            store,
            &row(&self.cx, self.id),
            now,
            PROBE_DEADLINE,
        )
        .unwrap();
        settle(&self.cx, store, &row(&self.cx, self.id), &reading).unwrap();
    }

    /// The watched events, oldest first, with their payloads.
    fn events(&self) -> Vec<(String, serde_json::Value)> {
        let mut all = self.cx.store().unwrap().events(self.id, 100).unwrap();
        all.sort_by_key(|e| e.id);
        all.into_iter()
            .filter(|e| WATCHED.contains(&e.kind.as_str()))
            .map(|e| (e.kind, e.payload))
            .collect()
    }

    fn kinds(&self) -> Vec<String> {
        self.events().into_iter().map(|(k, _)| k).collect()
    }

    fn session(&self) -> SessionState {
        row(&self.cx, self.id).session
    }
}

fn silent() -> FakeEngine {
    FakeEngine::new().probes(SESSION, Probe::TimedOut)
}

#[test]
fn a_box_silent_for_two_minutes_is_marked_unreachable_once_and_still_works() {
    let rig = rig(silent());
    rig.pass_after(60);
    assert!(rig.kinds().is_empty(), "a minute of silence is no outage");
    rig.pass_after(60);
    rig.pass_after(60);
    assert_eq!(rig.kinds(), ["agent_unreachable"]);
    let events = rig.events();
    let error = events
        .first()
        .unwrap()
        .1
        .get("error")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(error.contains("120 s"), "{error}");
    assert_eq!(rig.session(), SessionState::Working);
}

#[test]
fn a_box_silent_for_an_hour_keeps_its_session_and_open_turn() {
    let rig = rig(silent());
    steer(&rig.cx, rig.id);
    for _ in 0..60 {
        rig.pass_after(60);
    }
    assert_eq!(rig.kinds(), ["agent_unreachable"]);
    assert_eq!(rig.session(), SessionState::Working);
    assert!(rig
        .cx
        .store()
        .unwrap()
        .live_launch(rig.id)
        .unwrap()
        .is_some());
    assert_eq!(rig.cx.store().unwrap().sent(rig.id).unwrap().len(), 1);
}

#[test]
fn a_box_that_answers_again_is_restored_and_keeps_its_session() {
    let mut engine = FakeEngine::new().probes(SESSION, working(NOW));
    for _ in 0..3 {
        engine = engine.then_probes(SESSION, Probe::TimedOut);
    }
    let rig = rig(engine);
    for _ in 0..3 {
        rig.pass_after(60);
    }
    rig.pass_after(60);
    assert_eq!(rig.kinds(), ["agent_unreachable", "agent_link_restored"]);
    for _ in 0..6 {
        rig.pass_after(60);
    }
    assert_eq!(rig.kinds(), ["agent_unreachable", "agent_link_restored"]);
    assert_eq!(rig.session(), SessionState::Working);
}
