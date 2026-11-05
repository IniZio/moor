//! A supervisor whose controller goes away keeps serving for as long as it is gone, and carries
//! on under the same lease once the controller is back.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use moor_core::seam::clock::Clock as _;

use super::{start, Stop, World};

#[test]
fn a_controller_back_within_the_lease_finds_the_supervisor_still_serving() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.ledger.down();
    w.clock.sleep(Duration::from_secs(25));
    assert_eq!(run.tick(), None, "{}", w.stderr());
    assert!(
        UnixStream::connect(run.socket()).is_ok(),
        "the front still listens"
    );

    w.ledger.up();
    assert_eq!(run.tick(), None, "{}", w.stderr());
    let beat = w.row().lease_heartbeat.unwrap();
    assert_eq!(
        beat,
        w.clock.now(),
        "the lease was renewed once the controller came back"
    );
    assert_eq!(run.finish(Stop::Signal), 0);
}

#[test]
fn a_controller_gone_past_the_lease_finds_the_supervisor_still_serving_under_its_lease() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let owner = run.owner().to_owned();
    w.ledger.down();
    for _ in 0..150 {
        w.clock.sleep(Duration::from_secs(2));
        assert_eq!(run.tick(), None, "{}", w.stderr());
    }
    assert!(
        UnixStream::connect(run.socket()).is_ok(),
        "the front still listens"
    );

    w.ledger.up();
    assert_eq!(run.tick(), None, "{}", w.stderr());
    let row = w.row();
    assert_eq!(row.lease_owner.as_deref(), Some(owner.as_str()));
    assert_eq!(row.lease_heartbeat, Some(w.clock.now()));
    assert_eq!(run.finish(Stop::Signal), 0);
}

#[test]
fn a_supervisor_whose_key_was_replaced_stops_as_one_that_lost_its_lease() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.store
        .set_proxy_key(w.id, b"another", crate::proxy::ledger::LEDGER)
        .unwrap();
    assert_eq!(run.tick(), Some(Stop::LostLease));
}
