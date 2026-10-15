//! Tests for parking and reopening the tunnel, run against fakes.

use moor_core::seam::clock::Clock;

use super::super::tests::{World, FIRST_TUNNEL_PID};
use super::super::{start, Stop, TICK};
use crate::store::{TunnelAck, TunnelWant};
use crate::testing::driver::TunnelReply;

/// Asks the supervisor for `want`; the request counter advances on each call.
fn request(w: &World, want: TunnelWant) -> u64 {
    w.store.request_tunnel(w.id, want).unwrap()
}

#[test]
fn park_kills_the_tunnel_and_acks() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let seq = request(&w, TunnelWant::Parked);

    assert!(run.tick().is_none());

    assert_eq!(w.driver.dropped_tunnels(), [FIRST_TUNNEL_PID]);
    assert_eq!(
        w.store.tunnel_ack(w.id).unwrap(),
        Some(TunnelAck::Parked { seq })
    );
    assert_eq!(w.driver.tunnels().len(), 1);
}

#[test]
fn parked_supervisor_never_reopens_a_dead_tunnel() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    request(&w, TunnelWant::Parked);

    for _ in 0..20 {
        assert!(run.tick().is_none());
        w.clock.sleep(TICK);
    }

    assert_eq!(w.driver.tunnels().len(), 1);
    assert!(!w.stderr().contains("restarting"));
}

#[test]
fn parked_supervisor_still_beats_and_stops_when_the_task_closes() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    request(&w, TunnelWant::Parked);
    assert!(run.tick().is_none());
    let before = w.row().lease_heartbeat.unwrap();

    w.clock.sleep(TICK);
    assert!(run.tick().is_none());
    assert!(w.row().lease_heartbeat.unwrap() > before);

    w.close_task();
    assert!(matches!(run.tick(), Some(Stop::Closed)));
}

#[test]
fn up_reopens_on_the_same_ports_and_acks() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    request(&w, TunnelWant::Parked);
    assert!(run.tick().is_none());
    let seq = request(&w, TunnelWant::Up);

    assert!(run.tick().is_none());

    let calls = w.driver.tunnels();
    assert_eq!(calls.len(), 2);
    let (first, second) = (calls.first().unwrap(), calls.last().unwrap());
    assert_eq!(second.box_port, first.box_port);
    assert_eq!(second.host_port, first.host_port);
    assert_eq!(
        w.store.tunnel_ack(w.id).unwrap(),
        Some(TunnelAck::Up { seq })
    );
    assert!(run.tick().is_none());
    assert_eq!(w.driver.tunnels().len(), 2);
}

#[test]
fn failed_reopen_acks_the_error_and_keeps_the_proxy_running() {
    let w = World::new([
        TunnelReply::Ready,
        TunnelReply::EarlyExit {
            code: 1,
            stderr: "refused".into(),
        },
    ]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    request(&w, TunnelWant::Parked);
    assert!(run.tick().is_none());
    let seq = request(&w, TunnelWant::Up);

    assert!(run.tick().is_none());

    let ack = w.store.tunnel_ack(w.id).unwrap();
    assert!(
        matches!(&ack, Some(TunnelAck::Failed { seq: got, error })
            if *got == seq && error.contains("refused")),
        "{ack:?}"
    );
    assert!(w.row().lease_owner.is_some());
}
