//! Tests for the single agent read.

use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{
    AgentState, HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes, Source,
    STARTUP_GRACE,
};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::EngineError;
use moor_core::seam::surface::PaneRef;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use super::{read, settle, PaneLiveness, Reading};
use crate::cx::Cx;
use crate::store::{BoxRow, NewRun};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const NOW: Ts = Ts::from_millis(10_000);
const SESSION: &str = "sess-1";

fn cx(dir: &Path, engine: &Arc<FakeEngine>) -> (Cx, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new(dir)),
        proc.clone(),
    )
    .with_engine(engine.clone());
    (cx, proc)
}

/// A working Box `demo` with a ready machine and, when `session`, a live session.
fn working_box(cx: &Cx, session: bool) -> BoxId {
    let store = cx.store().unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
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
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        })
        .set(Fields {
            worktree: Field::Set("/wt".into()),
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    if session {
        store.begin_session(id, SESSION, None).unwrap();
    }
    id
}

/// Begin a run of `id` owned by pid 4242.
fn begin(cx: &Cx, id: BoxId) {
    cx.store()
        .unwrap()
        .begin_run(NewRun {
            box_id: id,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            sync: "none".into(),
            pid: 4242,
        })
        .unwrap();
}

fn row(cx: &Cx, id: BoxId) -> BoxRow {
    cx.store().unwrap().box_row(id).unwrap().unwrap()
}

fn is_working(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()
}

fn is_idle(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()
}

fn rec(kind: HookKind) -> HookRecord {
    HookRecord {
        kind,
        session_id: None,
        message: None,
        options: Vec::new(),
        at: Ts::from_millis(NOW.as_millis() - 1000),
    }
}

fn hook(kind: HookKind) -> Probes {
    Probes {
        hook: HookRead::Fresh(rec(kind)),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

fn state(r: &Reading) -> (AgentState, Source) {
    let a = r.activity.clone().expect("an agent");
    (a.state, a.source)
}

fn run_once(cx: &Cx, id: BoxId) -> Reading {
    read(cx, cx.store().unwrap(), &row(cx, id), NOW).unwrap()
}

#[test]
fn no_live_session_means_no_agent_and_no_probe() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, false);
    let r = run_once(&cx, id);
    assert_eq!(r.activity, None);
    assert!(!r.busy());
    assert_eq!(r.pane, PaneLiveness::Gone);
    assert!(r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 0);
}

#[test]
fn run_in_flight_without_a_session_is_busy_and_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, false);
    begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(r.activity, None);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 0);
}

#[test]
fn one_read_probes_once_and_decides_from_the_hook() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Idle)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Idle, Source::Hooks));
    assert_eq!(r.pane, PaneLiveness::Live);
    assert!(r.may_quiet);
    assert!(!r.busy());
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn run_in_flight_forces_working() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Idle)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(state(&r).0, AgentState::Working);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn run_in_flight_with_a_gone_pane_is_busy_and_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let gone = Probes {
        hook: HookRead::Absent,
        pane: PaneRead::Gone,
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, gone));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    begin(&cx, id);
    let r = run_once(&cx, id);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(r.pane, PaneLiveness::Gone);
}

#[test]
fn run_in_flight_with_an_unreachable_box_is_busy_and_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let store = cx.store().unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: "r".into(),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    store.begin_session(id, SESSION, None).unwrap();
    begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(r.activity, None);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 0);
}

#[test]
fn run_with_a_dead_runner_does_not_force_working() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Idle)));
    let (cx, proc) = cx(dir.path(), &engine);
    proc.dead(4242);
    let id = working_box(&cx, true);
    begin(&cx, id);
    assert_eq!(state(&run_once(&cx, id)).0, AgentState::Idle);
}

#[test]
fn fresh_hook_is_cached_and_a_timeout_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Busy)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    run_once(&cx, id);
    let cached = cx.store().unwrap().hook(id).unwrap().expect("cached");
    assert_eq!(cached.record.kind, HookKind::Busy);

    let dropped = Probes {
        hook: HookRead::TimedOut,
        pane: PaneRead::TimedOut,
    };
    let engine2 = Arc::new(FakeEngine::new().probes(SESSION, dropped));
    let cx = cx.with_engine(engine2);
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Working, Source::HooksCached));
    assert_eq!(cx.store().unwrap().hook(id).unwrap(), Some(cached));
}

#[test]
fn undecodable_cache_degrades_to_no_cache_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let dropped = Probes {
        hook: HookRead::TimedOut,
        pane: PaneRead::Absent,
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, dropped));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let raw = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO hook_cache (session, record, at) VALUES ((SELECT id FROM sessions WHERE box = ?1 AND ordinal = 1), '{\"kind\":\"nope\"}', 1)",
        [id.get()],
    )
    .unwrap();
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Unknown, Source::Timeout));
    let warnings = cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings.first().map(|w| w.code.as_str()),
        Some("agent_read_degraded")
    );
}

#[test]
fn cache_row_that_is_not_json_degrades_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let raw = rusqlite::Connection::open(dir.path().join("moor.db")).unwrap();
    raw.execute(
        "INSERT INTO hook_cache (session, record, at) VALUES ((SELECT id FROM sessions WHERE box = ?1 AND ordinal = 1), 'not json', 1)",
        [id.get()],
    )
    .unwrap();
    run_once(&cx, id);
    let warnings = cx.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings.first().map(|w| w.code.as_str()),
        Some("agent_read_degraded")
    );
}

#[test]
fn pane_liveness_tells_gone_from_exited_from_live() {
    let cases = [
        (PaneRead::Gone, PaneLiveness::Gone),
        (PaneRead::Absent, PaneLiveness::Gone),
        (PaneRead::TimedOut, PaneLiveness::Live),
        (
            PaneRead::Fresh(PaneObs {
                status: PaneStatus::Unknown,
                tail: String::new(),
                prompt: None,
                exit_marker: Some(3),
            }),
            PaneLiveness::Exited,
        ),
    ];
    for (pane, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let p = Probes {
            hook: HookRead::Absent,
            pane,
        };
        let engine = Arc::new(FakeEngine::new().probes(SESSION, p));
        let (cx, _) = cx(dir.path(), &engine);
        let id = working_box(&cx, true);
        assert_eq!(run_once(&cx, id).pane, want);
    }
    assert_eq!(PaneLiveness::Exited.as_str(), "exited");
}

#[test]
fn unformable_hook_path_degrades_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let e = EngineError::Box(DriverError::Failed {
        summary: "bad path".into(),
    });
    let engine = Arc::new(FakeEngine::new().fail_probe(SESSION, e));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Unknown, Source::Timeout));
    assert_eq!(cx.take_warnings().len(), 1);
}

fn idle_since(cx: &Cx, id: BoxId) -> Option<i64> {
    cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .query_row(
            "SELECT idle_since FROM boxes WHERE id = ?",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap()
}

fn reading(busy: bool, may_quiet: bool) -> Reading {
    Reading::new(None, PaneLiveness::Live, may_quiet, busy)
}

fn quiet(cx: &Cx, id: BoxId) {
    let d = Decision::new(id).session(SessionInput::AgentQuiet);
    cx.store().unwrap().apply(d).unwrap();
}

#[test]
fn settle_idles_a_working_box_that_may_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    assert_eq!(idle_since(&cx, id), None);
    settle(
        &cx,
        cx.store().unwrap(),
        &row(&cx, id),
        &reading(false, true),
    )
    .unwrap();
    assert!(is_idle(&row(&cx, id)));
    assert!(idle_since(&cx, id).is_some());
}

#[test]
fn settle_returns_an_idle_box_to_working_when_busy() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    quiet(&cx, id);
    assert!(idle_since(&cx, id).is_some());
    settle(
        &cx,
        cx.store().unwrap(),
        &row(&cx, id),
        &reading(true, false),
    )
    .unwrap();
    assert!(is_working(&row(&cx, id)));
    assert_eq!(idle_since(&cx, id), None);
}

#[test]
fn settle_leaves_a_working_box_that_may_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    settle(
        &cx,
        cx.store().unwrap(),
        &row(&cx, id),
        &reading(false, false),
    )
    .unwrap();
    assert!(is_working(&row(&cx, id)));
}

#[test]
fn settle_leaves_a_working_box_when_only_the_pane_reports_idle() {
    let cases = [
        (PaneStatus::Idle, AgentState::Idle),
        (
            PaneStatus::Blocked,
            AgentState::NeedsInput {
                question: None,
                options: Vec::new(),
            },
        ),
    ];
    for (status, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let p = Probes {
            hook: HookRead::Absent,
            pane: PaneRead::Fresh(PaneObs {
                status,
                tail: "> ".into(),
                prompt: None,
                exit_marker: None,
            }),
        };
        let engine = Arc::new(FakeEngine::new().probes(SESSION, p));
        let (cx, _) = cx(dir.path(), &engine);
        let id = working_box(&cx, true);
        let grace = i64::try_from(STARTUP_GRACE.as_millis()).unwrap();
        let later = Ts::from_millis(NOW.as_millis() + 2 * grace);
        let r = read(&cx, cx.store().unwrap(), &row(&cx, id), later).unwrap();
        assert_eq!(state(&r), (want, Source::Herdr), "{status:?}");
        assert!(!r.may_quiet, "{status:?}");
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
        assert!(is_working(&row(&cx, id)), "{status:?}");
        assert_eq!(idle_since(&cx, id), None);
    }
}

#[test]
fn settle_leaves_a_working_box_when_the_hook_read_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let p = Probes {
        hook: HookRead::TimedOut,
        pane: PaneRead::TimedOut,
    };
    let engine = Arc::new(FakeEngine::new().probes(SESSION, p));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
    assert!(is_working(&row(&cx, id)));
}

#[test]
fn settle_swallows_a_lost_race_and_keeps_the_new_state() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let stale = row(&cx, id);
    quiet(&cx, id);
    settle(&cx, cx.store().unwrap(), &stale, &reading(false, true)).unwrap();
    assert!(is_idle(&row(&cx, id)));

    let stale = row(&cx, id);
    let d = Decision::new(id).session(SessionInput::AgentBusy);
    cx.store().unwrap().apply(d).unwrap();
    settle(&cx, cx.store().unwrap(), &stale, &reading(true, false)).unwrap();
    assert!(is_working(&row(&cx, id)));
}

#[test]
fn the_stored_pane_reaches_the_probe() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, false);
    cx.store()
        .unwrap()
        .begin_session(id, SESSION, Some("p1"))
        .unwrap();
    run_once(&cx, id);
    assert_eq!(
        engine.probed_pane(SESSION).as_ref().map(PaneRef::as_str),
        Some("p1")
    );
}

#[test]
fn a_new_hook_session_id_is_stored_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut with_id = hook(HookKind::Idle);
    with_id.hook = HookRead::Fresh(HookRecord {
        session_id: Some("agent-1".into()),
        ..rec(HookKind::Idle)
    });
    let engine = Arc::new(FakeEngine::new().probes(SESSION, with_id));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    run_once(&cx, id);
    assert_eq!(row(&cx, id).agent_session_id.as_deref(), Some("agent-1"));
    let before = cx.store().unwrap().events(id, 100).unwrap().len();
    let updated = row(&cx, id).updated_at;
    run_once(&cx, id);
    assert_eq!(cx.store().unwrap().events(id, 100).unwrap().len(), before);
    assert_eq!(row(&cx, id).updated_at, updated);
}

mod exit;
mod needs_input;
mod turn;
