//! Tests for the controller's settle pass.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::{BoxId, BoxName, ProviderId};

use crate::controller::watch::{stream, Ask, Limits};
use crate::cx::Cx;
use crate::env::Env;
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
    Probes {
        hook: HookRead::Fresh(HookRecord {
            kind: HookKind::Idle,
            session_id: None,
            message: None,
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

/// A Working Task whose fake engine reports quiet, with the context that settles it.
fn quiet_agent(dir: &Path) -> (Cx, BoxId) {
    let driver = Arc::new(FakeDriver::new().exec_stdout([""]));
    let cx = engine_cx(dir, &driver, &Arc::new(FakeEngine::new()));
    let id = working(&cx, &driver, "a", "s-a");
    let engine = Arc::new(FakeEngine::new().probes("s-a", idle_hook(&cx)));
    (engine_cx(dir, &driver, &engine), id)
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
