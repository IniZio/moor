#![allow(clippy::indexing_slicing, clippy::panic, reason = "test assertions")]

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver};

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::user::Identity;
use moor_core::HoldOp;
use serde_json::Value;

use super::{start, Stop, World};
use crate::proxy::front::Hold;
use crate::proxy::supervisor::machine::hold_of;
use crate::store::TunnelWant;

/// What the machine runner was asked, with the connection it holds the forwards up on.
type Asked = Receiver<(Value, UnixStream)>;

/// Stands in for the machine runner's forward socket: runs `on_ask` for each request, answers
/// it `up` and holds its connection until the test shuts it.
fn runner_with(w: &World, on_ask: impl Fn() + Send + 'static) -> Asked {
    let dir = crate::runner_dir::of(w.dir.path());
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join(moor_client::forward::SOCKET)).unwrap();
    let (tx, asked) = mpsc::channel();
    std::thread::spawn(move || {
        // Held so the runner's side of each link stays open.
        #[allow(clippy::collection_is_never_read, reason = "held only to stay open")]
        let mut held = Vec::new();
        for conn in listener.incoming() {
            let mut conn = conn.unwrap();
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            on_ask();
            let _ = tx.send((
                serde_json::from_str(&line).unwrap(),
                conn.try_clone().unwrap(),
            ));
            conn.write_all(b"{\"reply\":\"up\"}\n").unwrap();
            held.push(conn);
        }
    });
    asked
}

fn runner(w: &World) -> Asked {
    runner_with(w, || {})
}

/// The Box ports the runner was last asked to serve on the machine.
fn last_forwards(asked: &Asked) -> Vec<u64> {
    let req = asked.try_iter().last().expect("the runner was asked").0;
    let ports = req["ports"].as_array().unwrap().iter();
    ports.map(|p| p[0].as_u64().unwrap()).collect()
}

/// A Box whose requester holds the machine `mini`, forwarding Box ports 8081 and 3001.
fn held_machine() -> (World, Asked) {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &["macos".to_owned()]).unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081, 3001],
        ..RepoMachine::default()
    };
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    let asked = runner(&w);
    (w, asked)
}

fn machine_kinds(w: &World) -> Vec<String> {
    let events = w.store.events(w.id, 50).unwrap();
    let kinds = events.into_iter().map(|e| e.kind);
    kinds.filter(|k| k.starts_with("machine_")).collect()
}

#[test]
fn the_first_tick_opens_the_forwards_and_reports_the_machine_attached() {
    let (w, asked) = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert!(run.tick().is_none());

    assert_eq!(machine_kinds(&w), ["machine_attached"]);
    assert_eq!(
        asked.try_iter().count(),
        1,
        "one held request serves every port"
    );
    assert!(run.tick().is_none());
    assert_eq!(
        machine_kinds(&w),
        ["machine_attached"],
        "a live link is reported once"
    );
}

#[test]
fn closing_the_box_ends_the_forwards_and_frees_the_machine() {
    let (w, _asked) = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());

    w.close_box();
    let stop = run.tick();
    assert!(matches!(stop, Some(Stop::Closed)));
    assert_eq!(run.finish(Stop::Closed), 0);

    assert!(
        w.store.box_machine(w.id).unwrap().is_none(),
        "the machine is free"
    );
    assert_eq!(machine_kinds(&w), ["machine_attached", "machine_detached"]);
    assert!(
        !w.driver.dropped_tunnels().is_empty(),
        "the tunnels were ended"
    );
}

#[test]
fn slot_one_forwards_each_port_moved_by_ten_at_the_same_number_on_both_ends() {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(2))
        .unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    let want = RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    };
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want), &[])
        .unwrap();
    let asked = runner(&w);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    assert_eq!(last_forwards(&asked), [8091]);
}

/// `held_machine` with `mini` taking `capacity` Boxes.
fn mini_of(capacity: u16) -> (World, moor_core::user::UserId, Asked) {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &["macos".to_owned()])
        .and_then(|m| m.with_capacity(capacity))
        .unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();
    let asked = runner(&w);
    (w, user, asked)
}

fn want_8081() -> RepoMachine {
    RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    }
}

fn machine_payloads(w: &World) -> Vec<serde_json::Value> {
    let events = w.store.events(w.id, 50).unwrap();
    let machine = events
        .into_iter()
        .filter(|e| e.kind.starts_with("machine_"));
    machine.map(|e| e.payload).collect()
}

#[test]
fn a_parked_tick_frees_the_slot_but_keeps_the_claim() {
    let (w, _, _asked) = mini_of(1);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());

    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());

    assert_eq!(machine_kinds(&w), ["machine_attached", "machine_detached"]);
    assert_eq!(machine_payloads(&w)[1]["asleep"], true);
    assert!(w.store.box_machine(w.id).unwrap().is_none());
    let claim = w.store.claimed_machine(w.id).unwrap().unwrap();
    assert_eq!(
        claim.state,
        moor_core::view::MachineHoldState::Asleep,
        "the Box keeps its claim on mini"
    );
}

#[test]
fn an_unparked_tick_takes_a_free_slot_and_forwards_its_ports() {
    let (w, user, asked) = mini_of(2);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert!(run.tick().is_none());

    let kinds = machine_kinds(&w);
    assert_eq!(kinds.last().map(String::as_str), Some("machine_attached"));
    let attached = machine_payloads(&w).pop().unwrap();
    assert_eq!(attached["ports"], serde_json::json!([8091]));
    assert_eq!(last_forwards(&asked), [8091]);
    assert_eq!(w.store.box_machine(w.id).unwrap().unwrap().slot, 1);
}

#[test]
fn a_full_machine_reports_waiting_once_and_wakes_when_a_slot_frees() {
    let (w, user, asked) = mini_of(1);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    let other = super::working_box(&w.store, &w.driver, "other");
    w.store
        .hold_machine(other, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert!((0..3).all(|_| run.tick().is_none()));

    let waiting = ["machine_attached", "machine_detached", "machine_waiting"];
    assert_eq!(machine_kinds(&w), waiting);
    let reason = machine_payloads(&w)[2]["reason"].to_string();
    assert!(reason.contains("Box other"), "{reason}");
    assert!(w.store.box_machine(w.id).unwrap().is_none());

    w.store.detach_machine(other).unwrap();
    assert!(run.tick().is_none());
    assert_eq!(machine_kinds(&w).last().unwrap(), "machine_attached");
    assert_eq!(last_forwards(&asked), [8081]);
}

fn missing_reason(w: &World) -> String {
    match hold_of(&w.store, w.id) {
        Hold::Missing { reason, .. } => reason,
        Hold::Held(_) => String::new(),
    }
}

#[test]
fn a_box_without_a_claim_is_told_its_repo_asks_for_none() {
    let w = World::new([]);
    assert!(missing_reason(&w).starts_with("this Box has no machine: "));
}

#[test]
fn an_asleep_claim_is_released_while_parked_and_waiting_once_asked_up() {
    let (w, _asked) = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    assert!(matches!(hold_of(&w.store, w.id), Hold::Held(h) if h.name == "mini"));

    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    assert_eq!(
        missing_reason(&w),
        "machine mini is released while this Box sleeps"
    );

    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();
    assert_eq!(
        missing_reason(&w),
        "machine mini has no free slot yet; this Box takes one as soon as another Box frees it"
    );
}

fn release(store: &crate::store::Store, id: moor_core::BoxId) {
    let (user, _) = store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    store
        .hold_machine(id, user, &HoldOp::Release, None, &[])
        .unwrap();
}

#[test]
fn a_released_claim_ends_the_forwards_on_the_next_tick() {
    let (w, _asked) = held_machine();
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    assert!(w.driver.dropped_tunnels().is_empty());

    release(&w.store, w.id);
    assert!(run.tick().is_none());

    assert!(
        !w.driver.dropped_tunnels().is_empty(),
        "the tunnels were ended"
    );
    assert!(run.tick().is_none());
    assert_eq!(machine_kinds(&w), ["machine_attached", "machine_detached"]);
}

#[test]
fn a_claim_released_while_its_forwards_open_is_not_attached() {
    let w = held_without_runner();
    let (store, id) = (w.store.clone(), w.id);
    let _asked = runner_with(&w, move || release(&store, id));
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert!(run.tick().is_none());

    assert_eq!(
        machine_kinds(&w),
        ["machine_detached"],
        "no attachment is recorded for a Box that holds nothing"
    );
    assert!(
        !w.driver.dropped_tunnels().is_empty(),
        "the tunnels were ended"
    );
}

#[test]
fn a_held_claim_reaches_the_machine_service_before_any_tick() {
    let (w, _asked) = held_machine();

    assert!(matches!(hold_of(&w.store, w.id), Hold::Held(h) if h.name == "mini"));
    assert!(machine_kinds(&w).is_empty(), "no tick has attached it");
}

/// `held_machine` before any runner listens.
fn held_without_runner() -> World {
    let w = World::new([]);
    let (user, _) = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None)
        .unwrap();
    let mini = UserMachine::parse("mini", &["macos".to_owned()]).unwrap();
    w.store.set_user_machine(user, &mini).unwrap();
    w.store
        .hold_machine(w.id, user, &HoldOp::repo(), Some(&want_8081()), &[])
        .unwrap();
    w
}

#[test]
fn the_runner_is_asked_for_the_held_machine_and_letting_go_is_noticed() {
    let (w, asked) = held_machine();
    let id = match hold_of(&w.store, w.id) {
        Hold::Held(h) => h.machine_id,
        Hold::Missing { reason, .. } => panic!("not held: {reason}"),
    };
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    assert!(run.tick().is_none());
    let (req, conn) = asked.try_recv().expect("the runner was asked to forward");
    assert_eq!(req["box_id"], w.id.get());
    assert_eq!(req["machine_id"], id);
    assert_eq!(
        req["ports"][0][0], 8081,
        "the Box port listens on the machine"
    );
    assert_eq!(machine_kinds(&w), ["machine_attached"]);

    conn.shutdown(Shutdown::Both).unwrap();
    assert!(run.tick().is_none());
    assert!(
        asked.try_recv().is_ok(),
        "the runner letting go is noticed and the forwards opened again"
    );
}

#[test]
fn a_removed_machine_is_refused() {
    let w = held_without_runner();
    let id = match hold_of(&w.store, w.id) {
        Hold::Held(h) => h.machine_id,
        Hold::Missing { reason, .. } => panic!("not held: {reason}"),
    };
    w.store
        .writer()
        .execute("DELETE FROM user_machines_v2 WHERE machine_id = ?", [id])
        .unwrap();
    let user = w
        .store
        .enroll(&Identity::parse("slack:T1:U1").unwrap(), None);
    let held = w.store.hold_machine(
        w.id,
        user.unwrap().0,
        &HoldOp::repo(),
        Some(&want_8081()),
        &[],
    );
    let removed = moor_core::HoldValue::Refused {
        reason: "machine mini was removed".to_owned(),
    };
    assert_eq!(held.unwrap(), removed);
    assert_eq!(missing_reason(&w), "machine mini was removed");
}
