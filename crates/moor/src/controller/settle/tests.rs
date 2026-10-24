//! Tests for the controller's settle pass.

use std::io::{self, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use moor_core::config::UserConfig;

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneObs, PaneRead, PaneStatus, Probes};
use moor_core::controller::WatchFilter;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::ExtraEvent;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::{AcquireReq, Driver, ExecOutcome};
use moor_core::user::Viewer;
use moor_core::{BoxId, BoxName, ProviderId, ShowReq};
use sha2::{Digest, Sha256};

use super::passes;
use crate::controller::drain::{serve_until, Drain, Handler};
use crate::controller::watch::{stream, Ask, Limits};
use crate::controller::Sink;
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
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
}

/// Adds a Working Box `name` with a live Box and an open session `session`.
fn working(cx: &Cx, driver: &FakeDriver, name: &str, session: &str) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let store = cx.store().unwrap();
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
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
            options: Vec::new(),
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

/// A Working Box whose fake engine reports quiet, with the context that settles it.
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
    let messages = Moor::from_cx(cx).settle_open(&|| false);
    assert!(messages.is_empty(), "{messages:?}");
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.session),
        (BoxLifecycle::Ready, SessionState::Idle)
    );
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
    Moor::from_cx(cx).settle_open(&|| false);
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
            since: 0,
            filter: WatchFilter::default(),
            follow: true,
            viewer: Viewer::Operator,
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
    let messages = Moor::from_cx(cx).settle_open(&|| false);
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
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&cx, &list).unwrap();
    let reader = view(dir.path(), &driver);
    let row = reader.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.session),
        (BoxLifecycle::Ready, SessionState::Idle)
    );
    let moor = Moor::from_cx(cx);
    let held = held_lock(&reader, id);
    assert!(moor.settle_open(&|| false).is_empty());
    assert_eq!(artifact_execs(&driver), 0);
    assert_eq!(stored(&reader, id), 0);
    drop(held);
    assert!(moor.settle_open(&|| false).is_empty());
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
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&cx, &list).unwrap();
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.session),
        (BoxLifecycle::Ready, SessionState::Idle)
    );
    assert_eq!(artifact_execs(&driver), 0);
    let messages = Moor::from_cx(cx).settle_open(&|| false);
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
    assert!(moor.settle_open(&|| false).is_empty());
    assert_eq!(artifact_execs(&driver), 1);
    assert!(moor.settle_open(&|| false).is_empty());
    assert!(moor.settle_open(&|| false).is_empty());
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
    assert!(moor.settle_open(&|| false).is_empty());
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.session),
        (BoxLifecycle::Ready, SessionState::Idle)
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
    let messages = moor.settle_open(&|| false);
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
    let messages = Moor::from_cx(cx).settle_open(&|| false);
    assert!(
        messages.iter().any(|m| m.contains("index script exited 1")),
        "{messages:?}"
    );
    let cx = view(dir.path(), &driver);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle, row.session),
        (BoxLifecycle::Ready, SessionState::Idle)
    );
    assert_eq!(stored(&cx, id), 0);
}

/// Time enough to pass the default `box.sleep_after`.
const LONG: Duration = Duration::from_mins(31);

const WAIT: Duration = Duration::from_secs(10);

/// Builds a fresh context over one state dir, clock and driver, as each pass wires one.
type Wire = Arc<dyn Fn() -> Cx + Send + Sync>;

/// Boxes `a` and `b` idle past `box.sleep_after`, with the wiring that settles them.
fn two_due(dir: &Path, driver: &Arc<FakeDriver>) -> Wire {
    let seed = engine_cx(dir, driver, &Arc::new(FakeEngine::new()));
    working(&seed, driver, "a", "s-a");
    working(&seed, driver, "b", "s-b");
    let hook = idle_hook(&seed);
    let engine = Arc::new(
        FakeEngine::new()
            .probes("s-a", hook.clone())
            .probes("s-b", hook),
    );
    let clock = Arc::new(FakeClock::new(seed.clock().now()));
    let (dir, driver, at) = (dir.to_owned(), Arc::clone(driver), Arc::clone(&clock));
    let wire: Wire = Arc::new(move || {
        Cx::fake_with_clock(
            &dir,
            driver.clone(),
            Arc::new(FakeSurface::new()),
            Arc::new(ScriptedProc::new()),
            UserConfig::default(),
            Arc::clone(&at),
        )
        .with_engine(engine.clone())
    });
    let idled = Moor::from_cx(wire()).settle_open(&|| false);
    assert!(idled.is_empty(), "{idled:?}");
    clock.sleep(LONG);
    wire
}

fn quiet() -> Sink {
    Arc::new(std::sync::Mutex::new(io::sink()))
}

fn sleeps(driver: &FakeDriver) -> usize {
    driver
        .calls()
        .iter()
        .filter(|c| c.starts_with("sleep "))
        .count()
}

#[test]
fn the_drain_waits_for_an_auto_sleep_in_flight_and_no_other_box_sleeps() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().exec_stdout(["", ""]));
    let wire = two_due(dir.path(), &driver);
    let (sleeping, asleep) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    driver.on_sleep(move || {
        sleeping.send(()).ok();
        released.lock().unwrap().recv_timeout(WAIT).ok();
    });
    let drain = Arc::new(Drain::default());
    let pass = Arc::clone(&wire);
    passes(
        Duration::from_millis(10),
        &drain,
        &quiet(),
        move |draining| Moor::from_cx(pass()).settle_open(draining),
    );
    asleep
        .recv_timeout(WAIT)
        .expect("a pass began an auto-sleep");
    let (wake, signal) = UnixStream::pair().unwrap();
    let (done, returned) = mpsc::channel();
    let serving = Arc::clone(&drain);
    thread::spawn(move || {
        let none: [(UnixListener, Handler); 0] = [];
        done.send(serve_until(&none, &wake, &serving, WAIT, &quiet()))
            .ok();
    });
    (&signal).write_all(&[1]).unwrap();
    while !drain.begun() {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        returned.recv_timeout(Duration::from_millis(300)).is_err(),
        "the drain returned while an auto-sleep was mid-flight"
    );
    release.send(()).unwrap();
    assert_eq!(returned.recv_timeout(WAIT).expect("drained"), 0);
    let store = wire();
    let mut lifecycles: Vec<BoxLifecycle> = store
        .store()
        .unwrap()
        .open_boxes()
        .unwrap()
        .iter()
        .map(|row| row.lifecycle)
        .collect();
    lifecycles.sort_by_key(|l| format!("{l:?}"));
    assert_eq!(lifecycles, [BoxLifecycle::Asleep, BoxLifecycle::Ready]);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        sleeps(&driver),
        1,
        "a Box began to sleep after the drain began"
    );
}

#[test]
fn no_settle_pass_starts_once_the_drain_began() {
    let drain = Arc::new(Drain::default());
    drain.begin();
    let (ran, seen) = mpsc::channel();
    passes(Duration::from_millis(10), &drain, &quiet(), move |_| {
        ran.send(()).ok();
        Vec::new()
    });
    assert!(
        seen.recv_timeout(Duration::from_millis(300)).is_err(),
        "a settle pass ran after the drain began"
    );
}

#[test]
fn a_drain_that_begins_after_the_settle_skips_the_artifact_collection() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    );
    let (cx, id) = quiet_agent_on(dir.path(), &driver);
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let draining = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0;
    Moor::from_cx(cx).settle_open(&draining);
    let cx = view(dir.path(), &driver);
    let kinds = kinds(&cx, id);
    assert!(kinds.iter().any(|k| k == "idled"), "{kinds:?}");
    assert_eq!(
        artifact_execs(&driver),
        0,
        "artifacts were read after the drain"
    );
}
