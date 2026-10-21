//! Tests for restarting the reverse tunnel, run against fakes with no real sleeps.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{
    AcquireReq, Acquired, BoxListing, Driver, DriverCaps, DriverError, ExecOutcome, ExecSpec,
    Released, SleepOutcome, TunnelHandle,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxId, BoxPath, ProviderId};

use super::super::tests::{unrestartable, World, FIRST_TUNNEL_PID};
use super::super::{start, Stop, LEASE, TICK};
use crate::store::{Lease, Reserve, Store};
use crate::testing::clock::FakeClock;
use crate::testing::driver::{FakeDriver, TunnelReply};
use crate::testing::rng::FakeRng;

/// Events of one kind recorded for the Box.
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
    /// The contested Box.
    box_id: BoxId,
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
            store.claim_lease(self.box_id, "rival", LEASE).unwrap(),
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
        box_id: w.id,
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
    let (path, clock, id) = (w.dir.path().join("moor.db"), w.clock.clone(), w.id);
    let slow = Slow {
        inner: &w.driver,
        clock: w.clock.clone(),
        took: LEASE,
        after: Box::new(move || {
            let other = Store::open(&path, clock.clone()).unwrap();
            let taken = other.claim_lease(id, "other-owner", LEASE).unwrap();
            assert!(matches!(taken, Lease::Claimed));
        }),
    };
    let mut deps = w.deps();
    deps.driver = &slow;

    assert_eq!(start(&deps, w.id).err(), Some(1));

    assert!(w.stderr().contains("took the lease"));
    assert_eq!(w.row().lease_owner.as_deref(), Some("other-owner"));
}

#[test]
fn slow_startup_extends_the_lease_before_the_row_is_marked_ready() {
    let w = World::new([]);
    let slow = Slow {
        inner: &w.driver,
        clock: w.clock.clone(),
        took: Duration::from_secs(20),
        after: Box::new(|| {}),
    };
    let mut deps = w.deps();
    deps.driver = &slow;

    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.clock.sleep(Duration::from_secs(20));
    let rival = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(rival, Lease::Held { .. }));
    assert_eq!(w.row().lease_owner.as_deref(), Some(run.owner()));
    assert!(w.row().ready);
}

/// Driver whose tunnel attempts take a long time on the fake clock.
struct Slow<'a> {
    /// Driver that does the work.
    inner: &'a FakeDriver,
    /// Clock advanced by every tunnel attempt.
    clock: Arc<FakeClock>,
    /// How long one attempt takes.
    took: Duration,
    /// Runs after each attempt has taken its time.
    after: Box<dyn Fn() + Send + Sync>,
}

impl Driver for Slow<'_> {
    fn caps(&self, p: &ProviderId) -> Result<DriverCaps, DriverError> {
        self.inner.caps(p)
    }

    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError> {
        self.inner.acquire(req)
    }

    fn planned_handle(&self, req: &AcquireReq) -> BoxHandle {
        self.inner.planned_handle(req)
    }

    fn release(&self, h: &BoxHandle) -> Result<Released, DriverError> {
        self.inner.release(h)
    }

    fn list(&self, p: &ProviderId) -> Result<Vec<BoxListing>, DriverError> {
        self.inner.list(p)
    }

    fn workdir(&self, h: &BoxHandle) -> Result<BoxPath, DriverError> {
        self.inner.workdir(h)
    }

    fn exec(&self, h: &BoxHandle, s: &ExecSpec, d: Deadline) -> Result<ExecOutcome, DriverError> {
        self.inner.exec(h, s, d)
    }

    fn copy_out(&self, h: &BoxHandle, f: &BoxPath, t: &Path) -> Result<(), DriverError> {
        self.inner.copy_out(h, f, t)
    }

    fn peek_file(&self, h: &BoxHandle, p: &BoxPath) -> Result<Option<String>, DriverError> {
        self.inner.peek_file(h, p)
    }

    fn put_file(
        &self,
        h: &BoxHandle,
        path: &BoxPath,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), DriverError> {
        self.inner.put_file(h, path, bytes, mode)
    }

    fn connect_argv(&self, h: &BoxHandle) -> Result<Vec<String>, DriverError> {
        self.inner.connect_argv(h)
    }

    fn reverse_tunnel(
        &self,
        h: &BoxHandle,
        bp: u16,
        socket: &Path,
    ) -> Result<Box<dyn TunnelHandle>, DriverError> {
        self.clock.sleep(self.took);
        (self.after)();
        self.inner.reverse_tunnel(h, bp, socket)
    }

    fn sleep(&self, h: &BoxHandle) -> Result<SleepOutcome, DriverError> {
        self.inner.sleep(h)
    }

    fn wake(&self, h: &BoxHandle) -> Result<(), DriverError> {
        self.inner.wake(h)
    }
}

#[test]
fn slow_restart_attempt_renews_the_lease_after_it_returns() {
    let w = World::new([]);
    let slow = Slow {
        inner: &w.driver,
        clock: w.clock.clone(),
        took: Duration::from_secs(29),
        after: Box::new(|| {}),
    };
    let mut deps = w.deps();
    deps.driver = &slow;
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    assert!(run.tick().is_none());
    w.clock.sleep(Duration::from_secs(5));

    let rival = w.store.claim_lease(w.id, "other-owner", LEASE).unwrap();
    assert!(matches!(rival, Lease::Held { .. }));
    assert_eq!(w.row().lease_owner.as_deref(), Some(run.owner()));
}

/// A step that runs 45 s on the fake clock and then has a rival reserve the row; the rival's
/// outcome lands in the returned slot.
fn outlasting_step(w: &World) -> (impl Fn() + Send + Sync, Arc<Mutex<Option<Reserve>>>) {
    let seen = Arc::new(Mutex::new(None));
    let (path, clock, id, out) = (
        w.dir.path().join("moor.db"),
        w.clock.clone(),
        w.id,
        seen.clone(),
    );
    // Real pauses let the renewal thread keep pace with the fake clock.
    let step = move || {
        for _ in 0..22 {
            clock.sleep(TICK);
            std::thread::sleep(Duration::from_millis(60));
        }
        let rival = Store::open(&path, clock.clone()).unwrap();
        let got = rival.reserve(id, &FakeRng::default(), LEASE);
        *out.lock().unwrap() = Some(got.unwrap());
    };
    (step, seen)
}

#[test]
fn slow_port_clear_keeps_the_lease_against_a_concurrent_reserve() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    let (outlast, seen) = outlasting_step(&w);
    w.driver.on_exec(outlast);

    w.driver.end_tunnel(FIRST_TUNNEL_PID);
    let stop = run.tick();

    let got = seen.lock().unwrap().take();
    assert!(matches!(got, Some(Reserve::Leased { .. })), "{got:?}");
    assert!(stop.is_none());
}

#[test]
fn slow_tunnel_open_keeps_the_lease_against_a_concurrent_reserve() {
    let w = World::new([]);
    let (outlast, seen) = outlasting_step(&w);
    let slow = Slow {
        inner: &w.driver,
        clock: w.clock.clone(),
        took: Duration::ZERO,
        after: Box::new(outlast),
    };
    let mut deps = w.deps();
    deps.driver = &slow;
    let started = start(&deps, w.id);

    let got = seen.lock().unwrap().take();
    assert!(matches!(got, Some(Reserve::Leased { .. })), "{got:?}");
    let run = started.map_err(|c| format!("exit {c}")).unwrap();
    assert_eq!(w.row().lease_owner.as_deref(), Some(run.owner()));
}
