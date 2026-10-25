//! Tests for `show` and `reconcile`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{
    AgentState, HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes, Source,
    STARTUP_GRACE,
};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput, SetupOutcome};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, SleepKind};
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, ProviderId, ShowReq, StartReq};

mod artifacts;
mod attention;
mod events;
mod pane;
mod pins;
mod target;

use super::run;
use crate::cx::Cx;
use crate::ops::common::activity::{self, Reading};
use crate::store::{BoxRow, NewRun, Store};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

type Parts = (BoxLifecycle, SessionState);
const WORKING: Parts = (BoxLifecycle::Ready, SessionState::Working);
const IDLE: Parts = (BoxLifecycle::Ready, SessionState::Idle);
fn parts(row: &BoxRow) -> Parts {
    (row.lifecycle, row.session)
}

/// Context over `driver` with state under `dir`.
fn cx(dir: &Path, driver: &Arc<FakeDriver>) -> Cx {
    Cx::fake(
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
}

/// Request showing everything.
fn req(box_name: Option<&str>, events: Option<u32>) -> ShowReq {
    ShowReq {
        target: box_name.map(|t| BoxName::parse_allow_bare(t).unwrap().into()),
        events,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    }
}

/// Insert a Box by name.
fn add(cx: &Cx, name: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(nt))
        .unwrap()
        .box_id
}

/// Give a Box a Ready box acquired from `driver`.
fn ready_box(cx: &Cx, driver: &FakeDriver, id: BoxId) -> moor_core::BoxHandle {
    let p = ProviderId::new("p");
    let got = driver
        .acquire(&AcquireReq {
            provider: p.clone(),
            box_id: id,
            name: BoxName::parse_allow_bare("demo").unwrap(),
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let fields = Fields {
        box_provider: Field::Set(p),
        ..Fields::default()
    };
    let store = cx.store().unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let acquired = BoxInput::Acquired {
        handle: got.handle.clone(),
    };
    store.apply(Decision::new(id).box_input(acquired)).unwrap();
    got.handle
}

/// The first row of a list-mode view.
fn first(v: &moor_core::view::ShowView) -> &moor_core::view::BoxView {
    v.boxes.as_ref().unwrap().first().unwrap()
}

/// Machine state of the only Box, as the view reports it.
fn box_state(cx: &Cx) -> BoxState {
    let v = run(cx, &req(None, None)).unwrap();
    first(&v).driver.as_ref().unwrap().state
}

#[test]
fn show_lists_open_boxes_with_no_selection() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    add(&cx, "a");
    add(&cx, "b");
    let closed = add(&cx, "c");
    let close = Decision::new(closed)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    cx.store().unwrap().apply(close).unwrap();
    let v = run(&cx, &req(None, None)).unwrap();
    let names: Vec<_> = v.boxes.iter().flatten().map(|t| t.name.clone()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(v.boxed.is_none() && v.events.is_none());
}

#[test]
fn sleep_label_follows_provider_sleep_support() {
    for (kind, after) in [(SleepKind::None, None), (SleepKind::Suspend, Some("30m"))] {
        let dir = tempfile::tempdir().unwrap();
        let driver = Arc::new(FakeDriver::new().sleep_kind(kind));
        let cx = cx(dir.path(), &driver);
        let id = add(&cx, "a");
        ready_box(&cx, &driver, id);
        let v = run(&cx, &req(Some("a"), None)).unwrap();
        let shown = v.boxed.unwrap().driver.unwrap().sleep.after;
        assert_eq!(shown.as_deref(), after);
        let v = run(&cx, &req(None, None)).unwrap();
        let listed = first(&v).driver.as_ref().unwrap().sleep.after.clone();
        assert_eq!(listed.as_deref(), after);
    }
}

#[test]
fn reconcile_marks_forgotten_box_gone() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    assert_eq!(box_state(&cx), BoxState::Ready);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Gone);
}

#[test]
fn list_failure_leaves_rows_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().fail_list(DriverError::Failed {
        summary: "boom".into(),
    }));
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
    let warned = cx.take_warnings();
    assert_eq!(warned.len(), 1);
    assert!(!warned.first().unwrap().message.contains("boom"));
}

#[test]
fn unsupported_list_is_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new().fail_list(DriverError::Unsupported("box provider has no inventory")),
    );
    let cx = cx(dir.path(), &driver);
    let id = add(&cx, "a");
    let handle = ready_box(&cx, &driver, id);
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
    assert!(cx.take_warnings().is_empty());
}

#[test]
fn named_box_returns_last_events() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    cx.store()
        .unwrap()
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Admitted))
        .unwrap();
    let v = run(&cx, &req(Some("a"), Some(1))).unwrap();
    assert!(v.boxes.is_none());
    assert_eq!(v.boxed.unwrap().name, "a");
    assert_eq!(v.events.unwrap().len(), 1);
}

#[test]
fn unknown_box_name_fails() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let e = run(&cx, &req(Some("nope"), None)).unwrap_err();
    assert!(e.error.to_string().contains("no Box nope"));
}

#[test]
fn box_shows_its_in_flight_run() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "a");
    let before = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(before.boxed.unwrap().run.is_none());
    let _claim = cx
        .store()
        .unwrap()
        .begin_run(NewRun {
            box_id: id,
            argv: vec!["make".into(), "test".into()],
            record: None,
            sha: None,
            pid: 4242,
        })
        .unwrap();
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    let r = v.boxed.unwrap().run.unwrap();
    assert_eq!(r.argv, ["make", "test"]);
    assert!(v.boxes.is_none());
    let listed = run(&cx, &req(None, None)).unwrap();
    assert!(first(&listed).run.is_some());
}

/// Context over `driver` with a scripted engine.
fn engine_cx(dir: &Path, driver: &Arc<FakeDriver>, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Add a working Box `name` with a box live in `driver`, and a live session
/// `session` when given.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: Option<&str>) -> BoxId {
    let id = add(cx, name);
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            box_id: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: "abc123".into(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let store = cx.store().unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    if let Some(s) = session {
        store.begin_session(id, s, None).unwrap();
    }
    id
}

/// Probes whose hook says `kind`, written just before the fake clock's now.
fn hook(cx: &Cx, kind: HookKind) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: Ts::from_millis(cx.clock().now().as_millis() - 1000),
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

#[test]
fn list_and_detail_report_the_same_agent() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let listed = first(&run(&cx, &req(None, None)).unwrap())
        .agent
        .clone()
        .unwrap();
    let detail = run(&cx, &req(Some("a"), None))
        .unwrap()
        .boxed
        .unwrap()
        .agent
        .unwrap();
    assert_eq!(listed, detail);
    assert_eq!(
        (detail.state.as_str(), detail.source.as_str()),
        ("idle", "hooks")
    );
}

#[test]
fn each_box_is_probed_once_per_command() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", Some("s-a"));
    working(&cx, &driver, "b", Some("s-b"));
    let engine = Arc::new(
        FakeEngine::new()
            .probes("s-a", hook(&cx, HookKind::Idle))
            .probes("s-b", hook(&cx, HookKind::Idle)),
    );
    let cx = engine_cx(dir.path(), &driver, &engine);
    run(&cx, &req(None, None)).unwrap();
    assert_eq!(engine.probe_count("s-a"), 1);
    assert_eq!(engine.probe_count("s-b"), 1);
    run(&cx, &req(Some("a"), Some(5))).unwrap();
    assert_eq!(engine.probe_count("s-a"), 2);
    assert_eq!(engine.probe_count("s-b"), 1);
}

#[test]
fn show_leaves_the_box_row_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let store = cx.store().unwrap();
    let before = store.box_row(id).unwrap();
    assert_eq!(parts(before.as_ref().unwrap()), WORKING);
    run(&cx, &req(Some("a"), None)).unwrap();
    assert_eq!(store.box_row(id).unwrap(), before);
}

/// Probes whose pane alone reports `status`, with no hook report.
fn pane_only(status: PaneStatus) -> Probes {
    Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Fresh(PaneObs {
            status,
            tail: "Allow this edit?".into(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

/// The `idle_since` column of a Box.
fn idle_since(store: &Store, id: BoxId) -> Option<i64> {
    store
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM boxes WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn list_settles_a_quiet_agent_to_idle() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    let v = run(&cx, &req(None, None)).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(parts(&row), IDLE);
    assert!(idle_since(cx.store().unwrap(), id).is_some());
    assert_eq!((first(&v).lifecycle, first(&v).session_state), IDLE);
}

#[test]
fn list_settles_a_busy_agent_to_working() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    cx.store()
        .unwrap()
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Busy)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    run(&cx, &req(None, None)).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(parts(&row), WORKING);
    assert!(idle_since(cx.store().unwrap(), id).is_none());
}

/// List a Working Box whose session is older than the startup grace and whose only
/// report is the pane's `status`; return the reading and the Box's state afterwards.
fn pane_only_reading(status: PaneStatus) -> (Reading, Parts) {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", Some("s-a"));
    let engine = Arc::new(FakeEngine::new().probes("s-a", pane_only(status)));
    let cx = engine_cx(dir.path(), &driver, &engine);
    cx.clock().sleep(STARTUP_GRACE + Duration::from_secs(60));
    run(&cx, &req(None, None)).unwrap();
    let store = cx.store().unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    let now = cx.clock().now();
    let reading = activity::read(&cx, store, &row, now, activity::PROBE_DEADLINE).unwrap();
    (reading, parts(&row))
}

#[test]
fn list_does_not_idle_on_a_pane_only_idle_report() {
    let (reading, state) = pane_only_reading(PaneStatus::Idle);
    let a = reading.activity.as_ref().unwrap();
    assert_eq!(a.source, Source::Herdr);
    assert_eq!(a.state, AgentState::Idle);
    assert!(!reading.may_quiet);
    assert_eq!(state, WORKING);
}

#[test]
fn list_does_not_idle_on_a_pane_only_blocked_report() {
    let (reading, state) = pane_only_reading(PaneStatus::Blocked);
    let a = reading.activity.as_ref().unwrap();
    assert_eq!(a.source, Source::Herdr);
    assert!(matches!(a.state, AgentState::NeedsInput { .. }));
    assert!(!reading.may_quiet);
    assert_eq!(state, WORKING);
}

#[test]
fn box_without_a_live_session_has_no_agent() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = engine_cx(dir.path(), &driver, &Arc::new(FakeEngine::new()));
    working(&cx, &driver, "a", None);
    let v = run(&cx, &req(Some("a"), None)).unwrap();
    assert!(v.boxed.unwrap().agent.is_none());
}

/// Context over `driver` whose git answers a repo at `/repo`, so `start` can run.
fn start_cx(dir: &Path, driver: &Arc<FakeDriver>) -> Cx {
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        Reply::Ok("/repo\n".into()),
    );
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok("abc123\n".into()),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok("/repo/.git\n".into()),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--git-path", "info/exclude"],
        Reply::Ok(format!("{}/exclude\n", dir.display())),
    );
    Cx::fake(dir, driver.clone(), Arc::new(FakeSurface::new()), proc)
        .with_engine(Arc::new(FakeEngine::new()))
}

#[test]
fn a_box_freshly_started_is_not_idled_by_the_next_list() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let cx = start_cx(dir.path(), &driver);
    let started = crate::ops::start::run(
        &cx,
        &serde_json::from_str::<StartReq>(r#"{"name": "demo", "goal": "ship it"}"#).unwrap(),
    )
    .unwrap();
    let s = &started.boxed;
    assert_eq!((s.lifecycle, s.session_state), WORKING);
    let v = run(&cx, &req(None, None)).unwrap();
    let t = first(&v);
    assert_eq!((t.lifecycle, t.session_state), WORKING);
    assert!(t.agent.is_some());
    let id = BoxId::new(started.boxed.id);
    assert!(cx.store().unwrap().live_session(id).unwrap().is_some());
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(parts(&row), WORKING);
}

/// A Box Idle since just now, with its agent still reporting idle.
fn idle_box(dir: &Path, driver: &Arc<FakeDriver>) -> (Cx, BoxId) {
    let cx = engine_cx(dir, driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, driver, "a", Some("s-a"));
    cx.store()
        .unwrap()
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook(&cx, HookKind::Idle)));
    (engine_cx(dir, driver, &engine), id)
}

const LONG: Duration = Duration::from_mins(31);

#[test]
fn list_sleeps_a_box_idle_past_the_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let (cx, id) = idle_box(dir.path(), &driver);
    cx.clock().sleep(LONG);
    let v = run(&cx, &req(None, None)).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Asleep);
    assert_eq!(first(&v).lifecycle, BoxLifecycle::Asleep);
    assert_eq!(first(&v).driver.as_ref().unwrap().state, BoxState::Asleep);
}

#[test]
fn list_leaves_a_recently_idle_box_up() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let (cx, id) = idle_box(dir.path(), &driver);
    run(&cx, &req(None, None)).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(parts(&row), IDLE);
}

#[test]
fn showing_one_box_does_not_sleep_it() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let (cx, id) = idle_box(dir.path(), &driver);
    cx.clock().sleep(LONG);
    run(&cx, &req(Some("a"), None)).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(parts(&row), IDLE);
    assert!(!driver.calls().iter().any(|c| c.starts_with("sleep ")));
}
