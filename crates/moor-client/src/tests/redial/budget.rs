//! How long a run of failed dials lasts: the budget, the refused window and missing paths.

#![allow(
    clippy::disallowed_methods,
    clippy::panic,
    reason = "tests time the redial with the wall clock and fail loudly"
)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{quick, socket, stale};
use crate::tests::show;
use crate::{redial, Attempt, Client, ClientError, GaveUp, Redial};
use moor_core::seam::clock::{Clock, Ts};

#[test]
fn a_budget_that_runs_out_names_the_budget_and_the_last_failure() {
    let dir = tempfile::tempdir().unwrap();
    stale(&socket(&dir));
    let redial = Redial {
        budget: Duration::from_millis(300),
        ..quick()
    };
    let err = Client::new(socket(&dir))
        .with_redial(redial)
        .request(show(), |_| {})
        .unwrap_err();
    let ClientError::GaveUp { budget, last } = &err else {
        panic!("{err}");
    };
    assert_eq!(*budget, Duration::from_millis(300));
    assert!(matches!(**last, ClientError::Connect { .. }), "{last}");
    let text = err.to_string();
    assert!(text.contains("300ms") && text.contains("c.sock"), "{text}");
}

#[test]
fn a_missing_socket_path_fails_at_once_under_the_default_redial() {
    let dir = tempfile::tempdir().unwrap();
    let began = Instant::now();
    let err = Client::new(socket(&dir))
        .request(show(), |_| {})
        .unwrap_err();
    assert!(matches!(err, ClientError::Connect { .. }), "{err}");
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn refused_connects_stop_after_the_refused_window_within_a_longer_budget() {
    let dir = tempfile::tempdir().unwrap();
    let sock = socket(&dir);
    stale(&sock);
    let redial = Redial {
        refused: Duration::from_millis(300),
        ..quick()
    };
    let began = Instant::now();
    let err = Client::new(&sock)
        .with_redial(redial)
        .request(show(), |_| {})
        .unwrap_err();
    let ClientError::GaveUp { budget, last } = &err else {
        panic!("{err}");
    };
    assert_eq!(*budget, Duration::from_millis(300));
    assert!(matches!(**last, ClientError::Connect { .. }), "{last}");
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "{:?}",
        began.elapsed()
    );
}

/// A clock that moves only when told to.
struct FakeClock {
    base: Instant,
    offset: Mutex<Duration>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
        }
    }

    fn elapsed(&self) -> Duration {
        *self.offset.lock().unwrap()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Ts {
        Ts::from_millis(0)
    }

    fn instant(&self) -> Instant {
        self.base + self.elapsed()
    }

    fn sleep(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

/// Runs `policy` against dials that each take a second and end as `each` says; returns how
/// it ended and the time it took. The thousandth dial fails, so a run that never spends its
/// budget ends too.
fn timed(
    policy: &Redial,
    each: fn() -> Attempt<(), &'static str>,
) -> (GaveUp<&'static str>, Duration) {
    let clock = FakeClock::new();
    let mut dials = 0;
    let ended = redial(
        policy,
        &clock,
        || {
            clock.sleep(Duration::from_secs(1));
            dials += 1;
            if dials == 1000 {
                return Attempt::Failed("the budget was never spent");
            }
            each()
        },
        |pause, _| clock.sleep(pause),
    )
    .unwrap_err();
    (ended, clock.elapsed())
}

#[test]
fn the_default_budget_is_sixty_seconds_of_wall_time_dials_included() {
    let (ended, took) = timed(&Redial::default(), || Attempt::Unrun("restarting"));
    assert!(
        matches!(ended, GaveUp::Budget { budget, .. } if budget == Duration::from_secs(60)),
        "{ended:?}"
    );
    assert!(
        took >= Duration::from_secs(60) && took <= Duration::from_secs(61),
        "{took:?}"
    );
}

#[test]
fn refused_connects_get_five_seconds_of_the_default_budget() {
    let (ended, took) = timed(&Redial::default(), || Attempt::Refused("refused"));
    assert!(
        matches!(ended, GaveUp::Budget { budget, .. } if budget == Duration::from_secs(5)),
        "{ended:?}"
    );
    assert!(
        took >= Duration::from_secs(5) && took <= Duration::from_secs(6),
        "{took:?}"
    );
}
