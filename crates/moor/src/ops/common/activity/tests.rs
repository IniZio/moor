//! Tests for the single agent read.

use moor_schema::tables::Boxes;

use crate::testing::db::column_where;
use std::path::Path;
use std::sync::Arc;

use moor_core::activity::{AgentState, Probe, Source, STARTUP_GRACE};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use super::{read, settle, PaneLiveness, Reading, PROBE_DEADLINE};
use crate::cx::Cx;
use crate::store::db::DbExt as _;
use crate::store::{BoxRow, FollowClaim, NewRun};
use crate::testing::agent::{exited, gone, idle, session as noted_session, working};
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;

const NOW: Ts = Ts::from_millis(10_000);
const SESSION: &str = "sess-1";

impl Reading {
    /// A reading of the given parts.
    pub(crate) const fn new(
        activity: Option<moor_core::activity::Activity>,
        pane: PaneLiveness,
        may_quiet: bool,
        run_in_flight: bool,
    ) -> Self {
        Self {
            activity,
            pane,
            may_quiet,
            run_in_flight,
            session: None,
            probed_pane: None,
            launch_failure: None,
            launch_overdue: None,
            agent_session: None,
            pane_text: None,
            said: None,
            read_failed: false,
            term: None,
        }
    }
}

fn cx(dir: &Path, engine: &Arc<FakeEngine>) -> (Cx, Arc<ScriptedProc>) {
    let proc = Arc::new(ScriptedProc::new());
    let cx = Cx::fake(dir, Arc::new(FakeDriver::new()), proc.clone()).with_engine(engine.clone());
    (cx, proc)
}

/// A working Box `demo` with a ready machine and, when `session`, a live session.
fn working_box(cx: &Cx, session: bool) -> BoxId {
    let store = cx.store().unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
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
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    if session {
        store.begin_session(id, SESSION, None).unwrap();
    }
    id
}

/// Begin a run of `id` owned by pid 4242; it stays in flight while the claim lives.
fn begin(cx: &Cx, id: BoxId) -> FollowClaim {
    cx.store()
        .unwrap()
        .begin_run(NewRun {
            box_id: id,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            pid: 4242,
        })
        .unwrap()
        .1
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

fn seen() -> Ts {
    Ts::from_millis(NOW.as_millis() - 1000)
}

fn state(r: &Reading) -> (AgentState, Source) {
    let a = r.activity.clone().expect("an agent");
    (a.state, a.source)
}

fn run_once(cx: &Cx, id: BoxId) -> Reading {
    read(cx, cx.store().unwrap(), &row(cx, id), NOW, PROBE_DEADLINE).unwrap()
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
    let _claim = begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(r.activity, None);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 0);
}

#[test]
fn one_read_probes_once_and_decides_from_the_status() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Idle, Source::Status));
    assert_eq!(r.pane, PaneLiveness::Live);
    assert!(r.may_quiet);
    assert!(!r.busy());
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn run_in_flight_forces_working() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let _claim = begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(state(&r).0, AgentState::Working);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 1);
}

#[test]
fn run_in_flight_with_a_gone_pane_is_busy_and_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, gone()));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let _claim = begin(&cx, id);
    let r = run_once(&cx, id);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(r.pane, PaneLiveness::Exited);
}

#[test]
fn run_in_flight_with_an_unreachable_box_is_busy_and_not_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let store = cx.store().unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("demo").unwrap(),
        repo: None,
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
    let _claim = begin(&cx, id);
    let r = run_once(&cx, id);
    assert_eq!(r.activity, None);
    assert!(r.busy());
    assert!(!r.may_quiet);
    assert_eq!(engine.probe_count(SESSION), 0);
}

#[test]
fn run_with_a_gone_follower_does_not_force_working() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    drop(begin(&cx, id));
    assert_eq!(state(&run_once(&cx, id)).0, AgentState::Idle);
}

#[test]
fn a_failed_read_is_unknown_and_marked_failed_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().fail_probe(SESSION));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    assert_eq!(state(&r), (AgentState::Unknown, Source::Timeout));
    assert!(r.read_failed);
    assert!(!r.may_quiet);
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
        (gone(), PaneLiveness::Exited),
        (exited(Some(3)), PaneLiveness::Exited),
        (working(seen()), PaneLiveness::Live),
        (Probe::TimedOut, PaneLiveness::Live),
    ];
    for (probe, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(FakeEngine::new().probes(SESSION, probe));
        let (cx, _) = cx(dir.path(), &engine);
        let id = working_box(&cx, true);
        assert_eq!(run_once(&cx, id).pane, want);
    }
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, false);
    assert_eq!(run_once(&cx, id).pane, PaneLiveness::Gone);
    assert_eq!(PaneLiveness::Exited.as_str(), "exited");
}

fn idle_since(cx: &Cx, id: BoxId) -> Option<i64> {
    cx.store()
        .unwrap()
        .reader()
        .unwrap()
        .row(
            column_where(Boxes::Table, Boxes::IdleSince, Boxes::Id, id.get()),
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
fn settle_leaves_a_working_box_when_the_agent_reports_no_status() {
    let grace = i64::try_from(STARTUP_GRACE.as_millis()).unwrap();
    let cases = [
        (NOW, AgentState::Starting),
        (
            Ts::from_millis(NOW.as_millis() + 2 * grace),
            AgentState::Unknown,
        ),
    ];
    for (at, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(FakeEngine::new());
        let (cx, _) = cx(dir.path(), &engine);
        let id = working_box(&cx, true);
        let r = read(&cx, cx.store().unwrap(), &row(&cx, id), at, PROBE_DEADLINE).unwrap();
        assert_eq!(state(&r), (want.clone(), Source::Terminal), "{want:?}");
        assert!(!r.may_quiet, "{want:?}");
        settle(&cx, cx.store().unwrap(), &row(&cx, id), &r).unwrap();
        assert!(is_working(&row(&cx, id)), "{want:?}");
        assert_eq!(idle_since(&cx, id), None);
    }
}

#[test]
fn settle_leaves_a_working_box_when_the_read_failed() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().fail_probe(SESSION));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    let r = run_once(&cx, id);
    assert!(r.read_failed);
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
fn a_new_note_session_id_is_stored_once() {
    let dir = tempfile::tempdir().unwrap();
    let with_id = noted_session(idle(seen()), "agent-1");
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

mod doing;
mod exit;
mod link;
mod needs_input;
mod said;
mod stall;
mod turn;
mod wait;
