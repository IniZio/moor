use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::rig::Rig;

#[test]
fn sigterm_lets_an_exec_in_flight_finish_and_tells_the_agent_to_relink() {
    let mut rig = Rig::start();
    rig.call("sync", json!({})).unwrap();
    let started = rig.copy().join("started");
    let (out, stopping) = std::thread::scope(|s| {
        let call = s.spawn(|| {
            rig.call(
                "exec",
                json!({"command": "touch started; sleep 2; echo done"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() {
            assert!(Instant::now() < deadline, "the exec never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        rig.sigterm();
        let stopping = Instant::now();
        (call.join().unwrap(), stopping)
    });
    let out = out.unwrap();
    assert_eq!(out["output"], "done\n", "{out}");
    assert_eq!(rig.exit_code(Duration::from_secs(10)), Some(0));
    assert!(
        stopping.elapsed() < Duration::from_secs(10),
        "the runner waited for the drain budget, not the stream"
    );
    let log = rig.agent_log.lock().unwrap().clone();
    assert!(
        log.iter().any(|l| l.contains("asked for a new link")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l.contains("handing over; dialing again in 1s")),
        "the stopping runner answered the next hello with relink: {log:?}"
    );
}

/// Forwards each connection to the port in `to` when it is accepted, as a Service in front of
/// runner pods does; the port it listens on.
fn forwarder(to: Arc<AtomicU16>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for near in listener.incoming() {
            let Ok(near) = near else { return };
            let Ok(far) = TcpStream::connect(("127.0.0.1", to.load(Ordering::SeqCst))) else {
                continue;
            };
            for (mut from, mut to) in [
                (near.try_clone().unwrap(), far.try_clone().unwrap()),
                (far, near),
            ] {
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from, &mut to);
                    let _ = to.shutdown(std::net::Shutdown::Write);
                });
            }
        }
    });
    port
}

#[test]
fn an_exec_in_flight_finishes_while_every_runner_rolls_under_it() {
    let mut old = Rig::start();
    let [mut next, mut last] = [Rig::start(), Rig::start()];
    for rig in [&mut next, &mut last] {
        if let Some(agent) = rig.machine_agent.take() {
            agent.shutdown_background();
        }
    }
    let to = Arc::new(AtomicU16::new(old.agents));
    old.agent_dials(forwarder(Arc::clone(&to)));
    let key = old.link.pubkey.lock().unwrap().clone();
    for rig in [&next, &last] {
        rig.link.pubkey.lock().unwrap().clone_from(&key);
    }
    old.call("sync", json!({})).unwrap();
    let started = old.copy().join("started");
    let out = std::thread::scope(|s| {
        let call = s.spawn(|| {
            old.call(
                "exec",
                json!({"command": "touch started; sleep 10; echo done"}),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !started.exists() {
            assert!(Instant::now() < deadline, "the exec never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        to.store(next.agents, Ordering::SeqCst);
        old.sigterm();
        next.online(true);
        to.store(last.agents, Ordering::SeqCst);
        next.sigterm();
        last.online(true);
        call.join().unwrap()
    });
    let log = old.agent_log.lock().unwrap().clone();
    let out = out.unwrap_or_else(|e| panic!("{e}; agent: {log:#?}"));
    assert_eq!(out["output"], "done\n", "{out}");
    assert_eq!(old.exit_code(Duration::from_secs(10)), Some(0));
    assert_eq!(next.exit_code(Duration::from_secs(10)), Some(0));
}
