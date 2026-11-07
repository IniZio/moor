//! The lock on a real database. Skipped without a test database.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use moor_core::seam::clock::Ts;
use moor_migrate::Outcome;

use super::{Claim, Claimant};
use crate::store::db::testing::postgres_target;
use crate::store::db::{ControllerLock, Target};
use crate::store::{Store, StoreError};
use crate::testing::clock::FakeClock;

/// A test's database, and a controller's state dir on it.
struct Fixture {
    dir: tempfile::TempDir,
    target: Target,
}

impl Fixture {
    fn new() -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let target = postgres_target(&dir.path().join("moor.db"))?;
        Some(Self { dir, target })
    }

    fn session(&self, me: &str) -> ControllerLock {
        ControllerLock::connect(&self.target, me).unwrap()
    }
}

/// Waits without ever being told to stop.
fn never_stopped(wait: Duration) -> bool {
    std::thread::sleep(wait);
    false
}

/// A claimant with no one to stop it, for `lost`.
fn claimant<'a>(me: &str, dir: &Path, lost: super::Lost) -> Claimant<'a> {
    Claimant {
        me: me.to_owned(),
        alive: dir.join("controller.alive"),
        stopped: &never_stopped,
        note: &|_| {},
        lost,
    }
}

#[test]
fn a_store_in_a_file_needs_no_claim() {
    let dir = tempfile::tempdir().unwrap();
    let lost = Arc::new(|_: &str| {});
    let claim = claimant("only", dir.path(), lost).claim(None).unwrap();
    assert_eq!(claim, Claim::Held);
    assert!(!dir.path().join("controller.alive").exists());
}

#[test]
fn a_taken_lock_stops_a_second_session_until_the_first_ends() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let mut first = fixture.session("first");
    assert!(first.try_take().unwrap());
    let mut second = fixture.session("second");
    assert!(!second.try_take().unwrap());
    assert_eq!(second.holder().unwrap().as_deref(), Some("first"));
    drop(first);
    second.take().unwrap();
    assert_eq!(second.holder().unwrap().as_deref(), Some("second"));
}

#[test]
fn the_lock_makes_nothing_and_the_migrations_run_under_it() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let mut lock = fixture.session("first");
    assert!(lock.try_take().unwrap());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open_postgres(&fixture.target, 1, fixture.dir.path(), clock).unwrap();
    assert_eq!(store.schema(), Outcome::Current);
    assert_eq!(lock.holder().unwrap().as_deref(), Some("first"));
}

#[test]
fn a_session_with_no_current_schema_is_refused_rather_than_trusted() {
    let Ok(url) = std::env::var("MOOR_TEST_POSTGRES_URL") else {
        return;
    };
    let sep = if url.contains('?') { '&' } else { '?' };
    let url = format!("{url}{sep}options=-c%20search_path%3Dmoor_no_such_schema");
    let target = Target::parse(&url).unwrap();
    let mut lock = ControllerLock::connect(&target, "first").unwrap();
    assert!(matches!(
        lock.try_take(),
        Err(StoreError::LockNotHeld { .. })
    ));
    assert!(matches!(lock.take(), Err(StoreError::LockNotHeld { .. })));
}

#[test]
fn a_lock_session_keeps_its_keepalives_and_no_server_timeout() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let config = fixture.target.lock_session("first");
    let config = config.config();
    assert_eq!(config.get_keepalives_idle(), Duration::from_secs(3));
    assert_eq!(config.get_keepalives_retries(), Some(3));
    let options = config.get_options().unwrap();
    for wanted in [
        "tcp_keepalives_idle=5",
        "tcp_keepalives_interval=2",
        "tcp_keepalives_count=3",
        "statement_timeout=0",
        "idle_session_timeout=0",
    ] {
        assert!(options.contains(wanted), "{options}");
    }
}

#[test]
fn a_holder_hears_its_session_end_at_once() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let (lost, losses) = mpsc::channel();
    let lost = Arc::new(move |why: &str| drop(lost.send(why.to_owned())));
    let me = "lock_hears_end";
    let claim = claimant(me, fixture.dir.path(), lost)
        .claim_on(&fixture.target)
        .unwrap();
    assert_eq!(claim, Claim::Held);
    assert!(losses.recv_timeout(Duration::from_millis(300)).is_err());
    ControllerLock::terminate(&fixture.target, me);
    let why = losses.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(why.contains("closed"), "{why}");
}

#[test]
fn a_waiter_told_to_stop_gets_no_lock() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let mut first = fixture.session("first");
    assert!(first.try_take().unwrap());
    let tries = AtomicUsize::new(0);
    let stop = |_: Duration| tries.fetch_add(1, Ordering::SeqCst) >= 2;
    let waiter = Claimant {
        stopped: &stop,
        ..claimant("second", fixture.dir.path(), Arc::new(|_: &str| {}))
    };
    assert_eq!(waiter.claim_on(&fixture.target).unwrap(), Claim::Stopped);
    assert_eq!(first.holder().unwrap().as_deref(), Some("first"));
}
