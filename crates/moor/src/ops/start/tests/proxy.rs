//! The per-task proxy supervisor is ensured before every agent launch.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_core::seam::proc::Tool;
use moor_core::secret::Secret;
use moor_core::{StartReq, TaskId};

use super::relaunch::{gone, started};
use super::{req, world, World};
use crate::ops::common::proxy::{log_path, LEASE, TAIL_LINES, WAIT};
use crate::ops::start::run;
use crate::store::{Fenced, ProxyPorts, Store};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::proc::{DaemonCall, Reply};

const HOST_PORT: u16 = 41000;

/// Simulates a healthy supervisor for each spawned daemon: it takes the lease on the
/// task named in the daemon's arguments and marks the proxy ready.
pub(super) fn supervise(state: PathBuf) -> impl Fn(&DaemonCall) + Send + Sync + 'static {
    move |d| {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
        let store = Store::open(&state.join("moor.db"), clock).unwrap();
        let task = TaskId::new(d.call.args.get(2).unwrap().parse().unwrap());
        let owner = "fake-supervisor";
        store.claim_lease(task, owner, LEASE).unwrap();
        let row = store.proxy(task).unwrap().unwrap();
        let ports = ProxyPorts {
            box_port: row.box_port,
            host_port: HOST_PORT,
        };
        let applied = store
            .mark_ready(task, owner, ports, d.pid, "fake-handle")
            .unwrap();
        assert_eq!(applied, Fenced::Applied);
    }
}

fn goal_req() -> StartReq {
    StartReq {
        goal: Some("ship it".into()),
        ..req("demo")
    }
}

/// A world whose engine records launches.
fn engine_world() -> (World, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new());
    let mut w = world(FakeDriver::new());
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

fn nonce_text(w: &World, id: TaskId) -> String {
    let row = w.cx.store().unwrap().proxy(id).unwrap().unwrap();
    String::from_utf8(row.nonce.expose().to_vec()).unwrap()
}

/// Replaces the supervisor with one that prints `lines` numbered lines and never gets ready.
fn hang_after_logging(w: &World, lines: usize) {
    w.proc.on_daemon(move |d| {
        let mut text = String::new();
        for n in 1..=lines {
            writeln!(text, "line {n}").unwrap();
        }
        std::fs::create_dir_all(d.log.parent().unwrap()).unwrap();
        std::fs::write(&d.log, text).unwrap();
    });
}

fn failure_step(w: &World, id: TaskId) -> (String, Option<String>) {
    let row = w.cx.store().unwrap().task(id).unwrap().unwrap();
    let f = row.launch_failure.unwrap();
    (f.step, f.tail)
}

fn elapsed(w: &World, from: Ts) -> Duration {
    Duration::from_millis(u64::try_from(w.cx.clock().now().as_millis() - from.as_millis()).unwrap())
}

#[test]
fn the_first_launch_spawns_one_supervisor_and_hands_the_agent_its_address() {
    let (w, engine) = engine_world();
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    let daemons = w.proc.daemons();
    assert_eq!(daemons.len(), 1);
    let d = daemons.first().unwrap();
    assert_eq!(d.call.tool, Tool::MoorSelf);
    assert_eq!(d.call.args, ["proxy", "serve", &id.get().to_string()]);
    assert_eq!(d.log, log_path(w.dir.path(), id));
    // Creating the log directory is the spawner's job, so it can pick the mode.
    assert!(!d.log.parent().unwrap().exists());
    let row = w.cx.store().unwrap().proxy(id).unwrap().unwrap();
    assert!(row.ready);
    let launches = engine.launches();
    let addr = launches.first().unwrap().proxy.as_ref().unwrap();
    assert_eq!(addr.box_port, row.box_port);
    assert_eq!(addr.nonce.expose(), row.nonce.expose());
}

#[test]
fn the_launch_carries_the_repo_origin_when_there_is_one() {
    let (w, engine) = engine_world();
    w.proc.on(
        Tool::Git,
        &["remote", "get-url", "origin"],
        Reply::Ok("git@github.com:o/r.git\n".into()),
    );
    run(&w.cx, &goal_req()).unwrap();
    let launches = engine.launches();
    let origin = launches.first().unwrap().origin.as_deref();
    assert_eq!(origin, Some("git@github.com:o/r.git"));
}

#[test]
fn the_launch_has_no_origin_without_a_remote() {
    let (w, engine) = engine_world();
    run(&w.cx, &goal_req()).unwrap();
    assert_eq!(engine.launches().first().unwrap().origin, None);
}

#[test]
fn a_relaunch_reuses_the_running_supervisor() {
    let (w, engine, _) = started();
    gone(&w);
    run(&w.cx, &req("demo")).unwrap();
    let launches = engine.launches();
    assert_eq!(launches.len(), 2);
    assert_eq!(w.proc.daemons().len(), 1);
    let [first, second] = [0, 1].map(|n| launches.get(n).unwrap().proxy.clone().unwrap());
    assert_eq!(first.box_port, second.box_port);
    assert_eq!(first.nonce.expose(), second.nonce.expose());
}

#[test]
fn a_supervisor_that_never_gets_ready_fails_the_launch_at_the_proxy_step() {
    let (w, engine) = engine_world();
    hang_after_logging(&w, 25);
    let start = w.cx.clock().now();
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    assert!(!v.agent.launched);
    assert_eq!(v.task.state, moor_core::domain::task::TaskState::Working);
    assert!(engine.launches().is_empty());
    assert!(elapsed(&w, start) >= WAIT);
    let (step, tail) = failure_step(&w, id);
    assert_eq!(step, "proxy");
    let expected: Vec<String> = (25 - TAIL_LINES + 1..=25)
        .map(|n| format!("line {n}"))
        .collect();
    let tail = tail.unwrap();
    assert_eq!(tail.lines().collect::<Vec<_>>(), expected);
    assert_eq!(expected.first().map(String::as_str), Some("line 6"));
}

#[test]
fn a_supervisor_leased_by_another_process_is_waited_for_not_spawned() {
    let (w, engine, id) = started();
    gone(&w);
    let store = w.cx.store().unwrap();
    assert!(store.remove_proxy(id).unwrap());
    let _ = store
        .reserve(id, &Secret::new(b"other".to_vec()), w.cx.rng(), LEASE)
        .unwrap();
    store.claim_lease(id, "other-supervisor", LEASE).unwrap();
    let start = w.cx.clock().now();
    let v = run(&w.cx, &req("demo")).unwrap();
    assert!(!v.agent.launched);
    assert_eq!(w.proc.daemons().len(), 1);
    assert_eq!(engine.launches().len(), 1);
    assert!(elapsed(&w, start) >= WAIT);
    assert_eq!(failure_step(&w, id).0, "proxy");
}

#[test]
fn the_nonce_stays_out_of_every_record_but_the_store_row_and_the_launch_address() {
    let (w, engine) = engine_world();
    let v = run(&w.cx, &goal_req()).unwrap();
    let id = TaskId::new(v.task.id);
    let nonce = nonce_text(&w, id);
    assert!(!nonce.is_empty());
    let daemons = w.proc.daemons();
    let d = daemons.first().unwrap();
    let mut seen = vec![format!("{:?}", d.call.args), format!("{:?}", d.call.env)];
    seen.push(format!("{:?}", engine.launches()));
    for call in w.proc.calls() {
        seen.push(format!("{:?} {:?}", call.args, call.env));
    }
    for e in w.cx.store().unwrap().events(id, 1000).unwrap() {
        seen.push(format!("{} {}", e.kind, e.payload));
    }
    seen.push(std::fs::read_to_string(&d.log).unwrap_or_default());
    for text in seen {
        assert!(!text.contains(&nonce), "{text}");
    }
}
