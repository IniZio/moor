//! A Proxy that an older moor left running with a TCP front, met by a controller that starts.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use super::socket::{ask, REFUSAL};
use super::{start, Stop, World, BOOT, LEASE};
use crate::proxy::adopt::reconcile;
use crate::proxy::detached::DetachedSupervisor;
use crate::store::ReadyProc;
use crate::testing::bridge::bridge;
use crate::testing::proc::ScriptedProc;
use crate::testing::rng::FakeRng;

/// The older moor's supervisor: alive, leading its group, in this boot.
const OLD_PID: u32 = 4242;
const OLD_OWNER: &str = "tcp-front-supervisor";

/// Records a ready row the way a moor build with a TCP front did, holding a live lease.
fn record_tcp_front(w: &World) -> u16 {
    let port = w.row().box_port;
    w.store.claim_lease(w.id, OLD_OWNER, LEASE).unwrap();
    let proc = ReadyProc {
        pid: OLD_PID,
        handle: "h",
        boot_id: Some(BOOT),
    };
    let _ = w.store.mark_ready(w.id, OLD_OWNER, port, proc).unwrap();
    rusqlite::Connection::open(w.dir.path().join("moor.db"))
        .unwrap()
        .execute("UPDATE proxy SET host_port = 4000", [])
        .unwrap();
    port
}

#[test]
fn an_older_moors_tcp_front_is_replaced_and_the_box_port_reaches_the_socket_front() {
    let w = Arc::new(World::new([]));
    let port = record_tcp_front(&w);
    let proc = Arc::new(ScriptedProc::new());
    proc.with_boot_id(BOOT);
    let (finish, finished) = mpsc::channel::<()>();
    let finished = Mutex::new(Some(finished));
    let supervisors = Arc::new(Mutex::new(Vec::new()));
    let (world, spawned) = (Arc::clone(&w), Arc::clone(&supervisors));
    proc.on_daemon(move |_| {
        let (w, finished) = (Arc::clone(&world), finished.lock().unwrap().take().unwrap());
        let (up, is_up) = mpsc::channel();
        spawned.lock().unwrap().push(thread::spawn(move || {
            let deps = w.deps();
            let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
            up.send(()).unwrap();
            let _ = finished.recv();
            run.finish(Stop::Signal)
        }));
        is_up.recv().unwrap();
    });
    let host = DetachedSupervisor::new(
        proc.clone(),
        w.clock.clone(),
        Arc::new(FakeRng::default()),
        Some(w.dir.path().to_path_buf()),
    )
    .keeping_nonce();

    let errors = reconcile(&host, proc.as_ref(), w.clock.clone(), w.dir.path());

    assert_eq!(errors, Vec::<String>::new());
    assert!(!w.store.heartbeat(w.id, OLD_OWNER).unwrap());
    let tunnels = w.driver.tunnels();
    let tunnel = tunnels.last().unwrap();
    assert_eq!(tunnel.box_port, port);
    assert!(ask(bridge(&tunnel.socket)).starts_with(REFUSAL));
    drop(finish);
    for supervisor in supervisors.lock().unwrap().drain(..) {
        assert_eq!(supervisor.join().unwrap(), 0);
    }
}
