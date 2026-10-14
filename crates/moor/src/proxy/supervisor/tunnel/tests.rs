//! Tests for restarting the reverse tunnel, run against fakes with no real sleeps.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moor_core::seam::clock::{Clock, Ts};
use moor_core::TaskId;

use super::super::tests::{unrestartable, World, FIRST_TUNNEL_PID, NONCE_TEXT, ORIGIN};
use super::super::{start, Stop, LEASE};
use crate::store::{Lease, Store};
use crate::testing::clock::FakeClock;
use crate::testing::driver::TunnelReply;

/// Events of one kind recorded for the task.
fn events_of(w: &World, kind: &str) -> usize {
    let events = w.store.events(w.id, 50).unwrap();
    events.iter().filter(|e| e.kind == kind).count()
}

/// Milliseconds the fake clock has advanced since `since`.
fn slept_since(w: &World, since: i64) -> i64 {
    w.clock.now().as_millis() - since
}

#[test]
fn crashed_tunnel_is_restarted_and_recorded_once() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let began = w.clock.now().as_millis();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);

    assert!(run.tick().is_none());
    assert_eq!(slept_since(&w, began), 1_000);
    assert_eq!(w.driver.tunnels().len(), 2);
    assert_eq!(events_of(&w, "tunnel_restarted"), 1);
    assert!(run.tick().is_none());
    assert_eq!(events_of(&w, "tunnel_restarted"), 1);
}

#[test]
fn five_failed_restarts_exit_one_keep_the_row_and_release_the_lease() {
    let w = World::new(unrestartable());
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let began = w.clock.now().as_millis();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let stop = run.tick();

    assert!(matches!(stop, Some(Stop::TunnelDead)));
    assert_eq!(slept_since(&w, began), 31_000);
    assert_eq!(w.driver.tunnels().len(), 6);
    assert_eq!(run.finish(Stop::TunnelDead), 1);
    assert_eq!(w.row().lease_owner, None);
    assert_eq!(events_of(&w, "tunnel_restarted"), 0);
    assert!(w.stderr().contains("did not restart after 5 attempts"));
    assert!(!w.stderr().contains(NONCE_TEXT));
}

#[test]
fn restart_on_the_third_attempt_resets_the_backoff() {
    let fail = || TunnelReply::EarlyExit {
        code: 1,
        stderr: "refused".into(),
    };
    let w = World::new([TunnelReply::Ready, fail(), fail()]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let began = w.clock.now().as_millis();
    assert!(run.tick().is_none());
    assert_eq!(slept_since(&w, began), 7_000);
    assert_eq!(events_of(&w, "tunnel_restarted"), 1);

    w.driver.end_tunnel(FIRST_TUNNEL_PID + 1);
    let began = w.clock.now().as_millis();
    assert!(run.tick().is_none());
    assert_eq!(slept_since(&w, began), 1_000);
    assert_eq!(events_of(&w, "tunnel_restarted"), 2);
}

/// Clock that lets a rival try to claim the lease after every sleep.
struct Rival {
    /// Underlying fake clock.
    inner: Arc<FakeClock>,
    /// Database the rival opens.
    db: PathBuf,
    /// The contested task.
    task: TaskId,
    /// Set once the rival wins the lease.
    won: Arc<AtomicBool>,
}

impl Clock for Rival {
    fn now(&self) -> Ts {
        self.inner.now()
    }

    fn instant(&self) -> Instant {
        self.inner.instant()
    }

    fn sleep(&self, d: Duration) {
        self.inner.sleep(d);
        let store = Store::open(&self.db, self.inner.clone()).unwrap();
        if matches!(
            store.claim_lease(self.task, "rival", LEASE).unwrap(),
            Lease::Claimed
        ) {
            self.won.store(true, Ordering::SeqCst);
        }
    }
}

#[test]
fn heartbeat_continues_through_the_backoff() {
    let fail = || TunnelReply::EarlyExit {
        code: 1,
        stderr: "refused".into(),
    };
    let replies = [TunnelReply::Ready, fail(), fail(), fail(), fail()];
    let w = World::new(replies);
    let won = Arc::new(AtomicBool::new(false));
    let clock = Arc::new(Rival {
        inner: w.clock.clone(),
        db: w.dir.path().join("moor.db"),
        task: w.id,
        won: won.clone(),
    });
    let deps = w.deps_with(clock);
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let began = w.clock.now().as_millis();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let stop = run.tick();

    assert!(stop.is_none());
    assert_eq!(slept_since(&w, began), 31_000);
    assert!(31_000 > i64::try_from(LEASE.as_millis()).unwrap());
    assert!(!won.load(Ordering::SeqCst));
    assert_eq!(events_of(&w, "tunnel_restarted"), 1);
}

#[test]
fn lost_lease_during_the_backoff_exits_without_touching_the_row() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.clock.sleep(LEASE);
    let taken = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(taken, Lease::Claimed));
    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let stop = run.tick();

    assert!(matches!(stop, Some(Stop::LostLease)));
    assert_eq!(w.driver.tunnels().len(), 1);
    assert_eq!(w.row().lease_owner.as_deref(), Some("other-owner"));
}

#[test]
fn lost_lease_at_startup_writes_a_stderr_line() {
    let w = World::new([]);
    let mut deps = w.deps();
    let (path, clock, id) = (w.dir.path().join("moor.db"), w.clock.clone(), w.id);
    deps.origin_of = Box::new(move |_: &Path| {
        clock.sleep(LEASE);
        let other = Store::open(&path, clock.clone()).unwrap();
        let taken = other.claim_lease(id, "other-owner", LEASE).unwrap();
        assert!(matches!(taken, Lease::Claimed));
        Some(ORIGIN.to_owned())
    });

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(w.stderr().contains("took the lease"));
    assert_eq!(w.row().lease_owner.as_deref(), Some("other-owner"));
}

#[test]
fn slow_startup_extends_the_lease_before_the_row_is_marked_ready() {
    let w = World::new([]);
    let mut deps = w.deps();
    let clock = w.clock.clone();
    deps.origin_of = Box::new(move |_: &Path| {
        clock.sleep(Duration::from_secs(20));
        Some(ORIGIN.to_owned())
    });

    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.clock.sleep(Duration::from_secs(20));
    let rival = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(rival, Lease::Held { .. }));
    assert_eq!(w.row().lease_owner.as_deref(), Some(run.owner()));
    assert!(w.row().ready);
}
