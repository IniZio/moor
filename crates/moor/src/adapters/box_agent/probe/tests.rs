use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus};
use moor_core::name::NameError;
use moor_core::seam::agent_profile::{AgentProfile, BoxSetup, EnvVar, Key, LaunchCtx, PromptMatch};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::engine::{Engine, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

use crate::adapters::box_agent::gh_release::fake::offline_release;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

struct Stub;

impl AgentProfile for Stub {
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord> {
        let kind = match raw {
            b"busy" => HookKind::Busy,
            b"idle" => HookKind::Idle,
            _ => return None,
        };
        Some(HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            at: Ts::from_millis(1),
        })
    }

    fn pane_prompt(&self, tail: &str) -> Option<String> {
        tail.strip_prefix("ask ").map(str::to_owned)
    }

    fn first_run_prompt(&self, _: &str) -> Option<PromptMatch> {
        None
    }

    fn argv(&self, _: &LaunchCtx) -> Vec<String> {
        Vec::new()
    }

    fn resume_argv(&self, _: &str, _: &LaunchCtx) -> Vec<String> {
        Vec::new()
    }

    fn env(&self, _: &LaunchCtx) -> Vec<EnvVar> {
        Vec::new()
    }

    fn preseed(&self, _: &LaunchCtx) -> Result<BoxSetup, NameError> {
        Ok(BoxSetup {
            files: Vec::new(),
            step: String::new(),
        })
    }

    fn hook_writer(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError> {
        self.preseed(ctx)
    }

    fn busy_marker(&self) -> &'static str {
        "busy"
    }

    fn menu_keys(&self, _: u8) -> Vec<Key> {
        Vec::new()
    }

    fn transcript_script(&self, _: &str) -> String {
        String::new()
    }
}

const DEADLINE: Deadline = Deadline(Duration::from_secs(3));

fn hook_path() -> BoxPath {
    BoxPath::under("/work/box-1", ".moor/agent-state.json").unwrap()
}

fn obs() -> PaneObs {
    PaneObs {
        status: PaneStatus::Idle,
        tail: "tail".to_owned(),
        prompt: None,
        exit_marker: None,
    }
}

fn session(pane: Option<&str>) -> SessionRef {
    SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        pane: pane.map(PaneRef::new),
        warnings: Vec::new(),
    }
}

fn live_driver(driver: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(driver);
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        cache: moor_core::cache::CachePlan::none(),
    })
    .unwrap();
    d
}

fn agent(d: &Arc<FakeDriver>, s: &Arc<FakeSurface>) -> BoxAgent {
    BoxAgent::new(
        d.clone(),
        s.clone(),
        Arc::new(Stub),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        offline_release(),
    )
}

fn surface_with(obs: Result<PaneObs, SurfaceError>) -> Arc<FakeSurface> {
    Arc::new(FakeSurface::new().script_pane_obs("p-1", obs))
}

#[test]
fn reads_the_hook_file_once_under_the_workdir() {
    let d = live_driver(FakeDriver::new().peek_text("box-1", &hook_path(), "busy"));
    let s = surface_with(Ok(obs()));
    agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(d.peeks(), [(BoxHandle::new("box-1"), hook_path())]);
}

#[test]
fn the_hook_file_read_is_bounded_by_the_probe_deadline() {
    let d = live_driver(FakeDriver::new().peek_text("box-1", &hook_path(), "busy"));
    let s = surface_with(Ok(obs()));
    let short = Deadline(Duration::from_millis(700));
    agent(&d, &s).probe(&session(Some("p-1")), short).unwrap();
    assert_eq!(d.batch_deadlines(), [short]);
}

#[test]
fn observes_the_pane_once() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Ok(obs()));
    agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(s.calls(), ["pane_obs p-1"]);
}

#[test]
fn returns_the_parsed_hook_and_the_pane_observation() {
    let d = live_driver(FakeDriver::new().peek_text("box-1", &hook_path(), "idle"));
    let s = surface_with(Ok(obs()));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(
        got.hook,
        HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            options: Vec::new(),
            at: Ts::from_millis(1),
        })
    );
    assert_eq!(got.pane, PaneRead::Fresh(obs()));
}

#[test]
fn unparsable_hook_file_reads_as_absent() {
    let d = live_driver(FakeDriver::new().peek_text("box-1", &hook_path(), "{\"kind\":"));
    let s = surface_with(Ok(obs()));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(got.hook, HookRead::Absent);
}

#[test]
fn missing_hook_file_reads_as_absent() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Ok(obs()));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(got.hook, HookRead::Absent);
}

#[test]
fn hook_read_timeout_reads_as_timed_out_and_the_pane_is_still_observed() {
    let d = live_driver(FakeDriver::new().fail_peek(DriverError::Timeout(Duration::from_secs(1))));
    let s = surface_with(Ok(obs()));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(got.hook, HookRead::TimedOut);
    assert_eq!(got.pane, PaneRead::Fresh(obs()));
}

#[test]
fn failed_hook_read_reads_as_timed_out_and_the_pane_is_still_observed() {
    for failure in [
        DriverError::Unavailable("down".to_owned()),
        DriverError::Failed {
            summary: "broken".to_owned(),
        },
    ] {
        let d = live_driver(FakeDriver::new().fail_peek(failure));
        let s = surface_with(Ok(obs()));
        let got = agent(&d, &s)
            .probe(&session(Some("p-1")), DEADLINE)
            .unwrap();
        assert_eq!(got.hook, HookRead::TimedOut);
        assert_eq!(got.pane, PaneRead::Fresh(obs()));
        assert_eq!(s.calls(), ["pane_obs p-1"]);
    }
}

#[test]
fn missing_pane_reads_as_gone() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Err(SurfaceError::NotFound));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(got.pane, PaneRead::Gone);
}

#[test]
fn pane_timeout_reads_as_timed_out() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Err(SurfaceError::Timeout));
    let got = agent(&d, &s)
        .probe(&session(Some("p-1")), DEADLINE)
        .unwrap();
    assert_eq!(got.pane, PaneRead::TimedOut);
}

#[test]
fn failed_pane_read_reads_as_timed_out() {
    for failure in [
        SurfaceError::Unavailable("gone".to_owned()),
        SurfaceError::Failed {
            summary: "broken".to_owned(),
        },
    ] {
        let d = live_driver(FakeDriver::new());
        let s = surface_with(Err(failure));
        let got = agent(&d, &s)
            .probe(&session(Some("p-1")), DEADLINE)
            .unwrap();
        assert_eq!(got.pane, PaneRead::TimedOut);
    }
}

#[test]
fn no_pane_reads_as_absent_without_touching_the_surface() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Ok(obs()));
    let got = agent(&d, &s).probe(&session(None), DEADLINE).unwrap();
    assert_eq!(got.pane, PaneRead::Absent);
    assert!(s.calls().is_empty());
}

#[test]
fn a_profile_that_does_not_say_otherwise_may_report_during_a_first_run_screen() {
    let d = live_driver(FakeDriver::new());
    let s = surface_with(Ok(obs()));
    assert!(!Stub.hooks_quiet_during_first_run());
    assert!(!agent(&d, &s).hooks_quiet_during_first_run());
}
