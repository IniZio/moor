use std::time::Duration;

use moor_core::seam::driver::DriverError;

use super::relinked_within;

const QUICK: (Duration, Duration, Duration) = (
    Duration::from_millis(300),
    Duration::from_millis(10),
    Duration::from_secs(5),
);

fn unlinked() -> DriverError {
    DriverError::Unavailable("machine 1 is not linked".to_owned())
}

/// A call that fails with each of `fails` in turn, then answers 7; the result and the calls made.
fn ask(fails: Vec<DriverError>) -> (Result<u8, DriverError>, usize) {
    let mut fails = fails.into_iter();
    let mut calls = 0;
    let got = relinked_within(QUICK, || {
        calls += 1;
        fails.next().map_or(Ok(7), Err)
    });
    (got, calls)
}

#[test]
fn a_call_made_while_the_link_is_down_is_asked_again_once_it_relinks() {
    let cut = DriverError::Unavailable("the stream ended: unexpected end of file".to_owned());
    let (got, calls) = ask(vec![cut, unlinked(), unlinked()]);
    assert_eq!(got.unwrap(), 7);
    assert_eq!(calls, 4);
}

#[test]
fn a_call_the_machine_refused_fails_at_once() {
    let refused = DriverError::Failed {
        summary: "machine 1 refused: no such Box".to_owned(),
    };
    let (got, calls) = ask(vec![refused, unlinked()]);
    assert!(matches!(got, Err(DriverError::Failed { .. })), "{got:?}");
    assert_eq!(calls, 1);
}

#[test]
fn a_link_down_past_the_wait_fails_with_what_the_link_said() {
    let (got, calls) = ask(vec![unlinked(); 1000]);
    assert!(
        matches!(&got, Err(DriverError::Unavailable(why)) if why.contains("not linked")),
        "{got:?}"
    );
    assert!(calls > 1 && calls < 1000, "{calls}");
}
