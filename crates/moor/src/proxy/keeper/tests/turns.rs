//! A keeper's calls into the Box take turns with ending and parking it: what waits for a clear
//! of the Box port in flight and what answers at once.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::World;
use crate::proxy::keeper::{lock, Keepers};
use crate::testing::driver::FakeDriver;

/// Holds each clear of the Box port until released; says when one began.
struct Gate {
    began: Receiver<()>,
    release: mpsc::Sender<()>,
}

fn gate_clears(driver: &FakeDriver) -> Gate {
    let (began_tx, began) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let (began_tx, release_rx) = (Mutex::new(began_tx), Mutex::new(release_rx));
    driver.on_exec_of("sshd", move || {
        began_tx.lock().unwrap().send(()).ok();
        release_rx.lock().unwrap().recv().ok();
    });
    Gate { began, release }
}

#[test]
fn up_and_park_answer_while_the_tunnel_opens() {
    let w = World::new([]);
    let gate = gate_clears(&w.driver);
    let kept = Arc::new(w.kept(true));
    let ticking = Arc::clone(&kept);
    let tick = std::thread::spawn(move || ticking.tick());
    gate.began.recv().unwrap();
    let (done_tx, done) = mpsc::channel();
    let asking = Arc::clone(&kept);
    std::thread::spawn(move || {
        let up = asking.up();
        asking.park();
        done_tx.send(up).ok();
    });
    let answered = done.recv_timeout(Duration::from_secs(5));
    gate.release.send(()).unwrap();
    assert_eq!(answered, Ok(false), "up and park did not wait for the open");
    assert!(tick.join().unwrap());
    assert!(!kept.up(), "a tunnel opened for a parked Box is not kept");
    assert_eq!(
        w.driver.tunnels().len(),
        w.driver.dropped_tunnels().len(),
        "every tunnel opened was closed"
    );
}

#[test]
fn an_ended_keeper_finishes_its_open_before_end_returns_and_keeps_nothing() {
    let w = World::new([]);
    let gate = gate_clears(&w.driver);
    let keepers: &'static Keepers = Box::leak(Box::default());
    let kept = Arc::new(w.kept(true));
    lock(&keepers.kept).insert(w.id, Arc::clone(&kept));
    let ticking = Arc::clone(&kept);
    let tick = std::thread::spawn(move || ticking.tick());
    gate.began.recv().unwrap();
    let (ended_tx, ended) = mpsc::channel();
    let id = w.id;
    std::thread::spawn(move || {
        drop(keepers.end(id));
        ended_tx.send(()).ok();
    });
    assert!(
        ended.recv_timeout(Duration::from_millis(300)).is_err(),
        "end waits for the open in flight"
    );
    gate.release.send(()).unwrap();
    ended.recv_timeout(Duration::from_secs(5)).unwrap();
    tick.join().unwrap();
    drop(gate);
    assert_eq!(
        w.driver.tunnels().len(),
        w.driver.dropped_tunnels().len(),
        "the ended keeper keeps no tunnel"
    );
    assert!(!kept.tick(), "an ended keeper does not tick again");
    assert_eq!(w.driver.tunnels().len(), 1, "and opens nothing more");
}

#[test]
fn ending_the_keeper_of_a_closed_box_does_not_wait_for_the_clear_in_flight() {
    let w = World::new([]);
    let gate = gate_clears(&w.driver);
    let keepers: &'static Keepers = Box::leak(Box::default());
    let kept = Arc::new(w.kept(true));
    lock(&keepers.kept).insert(w.id, Arc::clone(&kept));
    let ticking = Arc::clone(&kept);
    let tick = std::thread::spawn(move || ticking.tick());
    gate.began.recv().unwrap();
    w.close_box();
    let (ended_tx, ended) = mpsc::channel();
    let id = w.id;
    std::thread::spawn(move || {
        drop(keepers.end(id));
        ended_tx.send(()).ok();
    });
    let returned = ended.recv_timeout(Duration::from_secs(5));
    gate.release.send(()).unwrap();
    assert_eq!(returned, Ok(()), "end returned while the clear still ran");
    tick.join().unwrap();
    drop(gate);
    assert_eq!(
        w.driver.tunnels().len(),
        w.driver.dropped_tunnels().len(),
        "the ended keeper keeps no tunnel"
    );
}
