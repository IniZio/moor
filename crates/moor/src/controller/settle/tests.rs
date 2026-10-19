//! Tests for the controller's settle pass.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::ExtraEvent;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, ExecOutcome};
use moor_core::{BoxId, BoxName, ProviderId, ShowReq};
use sha2::{Digest, Sha256};

use crate::controller::watch::{stream, Ask, Limits};
use crate::cx::Cx;
use crate::env::Env;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::show;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use crate::Moor;

/// Context over `driver` with state under `dir` and a scripted engine.
fn engine_cx(dir: &Path, driver: &Arc<FakeDriver>, engine: &Arc<FakeEngine>) -> Cx {
    Cx::fake(
        dir,
        dir,
        driver.clone(),
        Arc::new(FakeSurface::new(dir)),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Adds a Working Task `name` with a live Box and an open session `session`.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: &str) -> BoxId {
    let nt = NewTask {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: "r".into(),
    };
    let store = cx.store().unwrap();
    let id = store.apply(Decision::insert(nt)).unwrap().task;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: BoxName::parse_allow_bare(name).unwrap(),
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            worktree: Field::Set("/wt".into()),
            box_workdir: Field::Set("/work".into()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    store.begin_session(id, session, None).unwrap();
    id
}

/// Probes whose hook says the agent is idle, written just before the fake clock's now.
fn idle_hook(cx: &Cx) -> Probes {
    idle_hook_at(Ts::from_millis(cx.clock().now().as_millis() - 1000))
}

/// Probes whose hook says the agent is idle, written at `at`.
fn idle_hook_at(at: Ts) -> Probes {
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
            at,
        }),
        pane: PaneRead::Fresh(PaneObs {
            status: PaneStatus::Unknown,
            tail: String::new(),
            prompt: None,
            exit_marker: None,
        }),
    }
}

/// A Working Task whose fake engine reports quiet, with the context that settles it.
fn quiet_agent(dir: &Path) -> (Cx, BoxId) {
    quiet_agent_on(dir, &Arc::new(FakeDriver::new().exec_stdout([""])))
}

/// A second context over the same state, for reading what a `Moor` wrote.
fn view(dir: &Path, driver: &Arc<FakeDriver>) -> Cx {
    engine_cx(dir, driver, &Arc::new(FakeEngine::new()))
}

/// Like `quiet_agent`, over the given `driver`.
fn quiet_agent_on(dir: &Path, driver: &Arc<FakeDriver>) -> (Cx, BoxId) {
    agent_with_hook(dir, driver, None)
}

/// Like `quiet_agent_on`, with the idle hook written at `at` when given.
fn agent_with_hook(dir: &Path, driver: &Arc<FakeDriver>, at: Option<Ts>) -> (Cx, BoxId) {
    let cx = engine_cx(dir, driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, driver, "a", "s-a");
    let hook = at.map_or_else(|| idle_hook(&cx), idle_hook_at);
    let engine = Arc::new(FakeEngine::new().probes("s-a", hook));
    (engine_cx(dir, driver, &engine), id)
}

/// A writer that accepts bytes until the `idled` frame arrives, then fails.
struct StopAtIdled(Vec<u8>, Arc<std::sync::Mutex<String>>);

impl Write for StopAtIdled {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(buf);
        let text = String::from_utf8_lossy(&self.0).into_owned();
        let done = text.contains(r#""kind":"idled""#);
        *self.1.lock().unwrap() = text;
        if done {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn pass_settles_a_quiet_agent_without_any_client() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, id) = quiet_agent(dir.path());
    let messages = Moor::from_cx(cx).settle_open();
    assert!(messages.is_empty(), "{messages:?}");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let row = store.task(id).unwrap().unwrap();
    assert_eq!(row.state, TaskState::Idle);
    let kinds: Vec<String> = store
        .events_after(0, 100)
        .unwrap()
        .iter()
        .map(|e| e.row.kind.as_str().to_owned())
        .collect();
    assert!(kinds.iter().any(|k| k == "idled"), "{kinds:?}");
}

#[test]
fn stream_delivers_idle_without_any_show() {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _) = quiet_agent(dir.path());
    Moor::from_cx(cx).settle_open();
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().unwrap()),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    let limits = Limits {
        buffer: 64,
        poll: Duration::from_millis(10),
        batch: 10,
    };
    let seen = Arc::default();
    stream(
        &mut StopAtIdled(Vec::new(), Arc::clone(&seen)),
        &env,
        Ask {
            follow: true,
            ..Ask::default()
        },
        limits,
    );
    let text = seen.lock().unwrap().clone();
    assert!(text.contains(r#""kind":"idled""#), "{text}");
}

const BODY: &[u8] = b"hello";

fn listing() -> String {
    format!("notes.txt\t{}\t{:x}\n", BODY.len(), Sha256::digest(BODY))
}

fn kinds(cx: &Cx, id: BoxId) -> Vec<String> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn artifact_execs(driver: &FakeDriver) -> usize {
    let execs = driver.execs();
    execs
        .iter()
        .filter(|(_, spec, _)| spec.argv.iter().any(|a| a.contains(".moor/artifacts")))
        .count()
}

fn stored(cx: &Cx, id: BoxId) -> usize {
    cx.store().unwrap().artifacts(id).unwrap().len()
}

#[test]
fn an_agent_that_idles_has_its_artifacts_collected() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    );
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let messages = Moor::from_cx(cx).settle_open();
    assert!(messages.is_empty(), "{messages:?}");
    let cx = view(dir.path(), &driver);
    let kinds = kinds(&cx, id);
    assert!(kinds.iter().any(|k| k == "idled"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "artifact_published"), "{kinds:?}");
    assert_eq!(stored(&cx, id), 1);
}

fn held_lock(cx: &Cx, id: BoxId) -> BoxLock {
    BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "close",
        Duration::ZERO,
    )
    .unwrap()
}

#[test]
fn a_held_box_lock_skips_the_collection() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    );
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let list = ShowReq {
        task: None,
        events: None,
        agent: false,
        reports: false,
        pane: false,
    };
    show::run(&cx, &list).unwrap();
    let reader = view(dir.path(), &driver);
    assert_eq!(
        reader.store().unwrap().task(id).unwrap().unwrap().state,
        TaskState::Idle
    );
    let moor = Moor::from_cx(cx);
    let held = held_lock(&reader, id);
    assert!(moor.settle_open().is_empty());
    assert_eq!(artifact_execs(&driver), 0);
    assert_eq!(stored(&reader, id), 0);
    drop(held);
    assert!(moor.settle_open().is_empty());
    assert_eq!(artifact_execs(&driver), 1);
    assert_eq!(stored(&reader, id), 1);
}

#[test]
fn an_idle_written_by_show_is_collected_by_the_next_pass() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    );
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let list = ShowReq {
        task: None,
        events: None,
        agent: false,
        reports: false,
        pane: false,
    };
    show::run(&cx, &list).unwrap();
    assert_eq!(
        cx.store().unwrap().task(id).unwrap().unwrap().state,
        TaskState::Idle
    );
    assert_eq!(artifact_execs(&driver), 0);
    let messages = Moor::from_cx(cx).settle_open();
    assert!(messages.is_empty(), "{messages:?}");
    let cx = view(dir.path(), &driver);
    assert!(kinds(&cx, id).iter().any(|k| k == "artifact_published"));
    assert_eq!(stored(&cx, id), 1);
}

#[test]
fn a_pass_with_no_new_idle_reads_no_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    );
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let moor = Moor::from_cx(cx);
    assert!(moor.settle_open().is_empty());
    assert_eq!(artifact_execs(&driver), 1);
    assert!(moor.settle_open().is_empty());
    assert!(moor.settle_open().is_empty());
    assert_eq!(artifact_execs(&driver), 1);
    assert_eq!(stored(&view(dir.path(), &driver), id), 1);
}

#[test]
fn a_steered_turn_on_an_idle_box_ends_and_collects_in_one_pass() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().copy_text(BODY));
    let later = Some(Ts::from_millis(i64::MAX / 4));
    let (cx, id) = agent_with_hook(dir.path(), &driver, later);
    let moor = Moor::from_cx(cx);
    let cx = view(dir.path(), &driver);
    assert!(moor.settle_open().is_empty());
    assert_eq!(
        cx.store().unwrap().task(id).unwrap().unwrap().state,
        TaskState::Idle
    );
    assert_eq!(stored(&cx, id), 0);
    let steered = ExtraEvent::Steered {
        session: "s-a".to_owned(),
        message: "go".to_owned(),
        submit_retried: false,
    };
    cx.store()
        .unwrap()
        .apply(Decision::new(id).event(steered))
        .unwrap();
    driver.push_artifact_index(listing());
    let messages = moor.settle_open();
    assert!(messages.is_empty(), "{messages:?}");
    let kinds = kinds(&cx, id);
    let at = |kind: &str| kinds.iter().position(|k| k == kind);
    let (done, published) = (at("turn_done"), at("artifact_published"));
    assert!(done.is_some(), "{kinds:?}");
    assert!(done < published, "{kinds:?}");
    assert_eq!(stored(&cx, id), 1);
}

#[test]
fn a_failed_index_warns_and_the_box_still_idles() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })]));
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let messages = Moor::from_cx(cx).settle_open();
    assert!(
        messages.iter().any(|m| m.contains("index script exited 1")),
        "{messages:?}"
    );
    let cx = view(dir.path(), &driver);
    assert_eq!(
        cx.store().unwrap().task(id).unwrap().unwrap().state,
        TaskState::Idle
    );
    assert_eq!(stored(&cx, id), 0);
}
