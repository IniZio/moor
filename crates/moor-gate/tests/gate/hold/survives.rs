//! A held request outlives what happens to the dial and to the gate itself while it waits.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use super::{answered_ok, held_served, held_with, holding, Answer, FRONT_KEY, HOLD, LONG};

#[test]
fn a_dial_that_gets_no_answer_does_not_use_up_the_hold() {
    let w = held_with(FRONT_KEY, HOLD);
    let session = w.session();
    *w.fake.answer.lock().unwrap() = Answer::Silent;
    let began = Instant::now();
    std::thread::scope(|scope| {
        let call = scope.spawn(|| w.status_of(session.as_deref(), "repo/silent"));
        std::thread::sleep(Duration::from_millis(500));
        *w.fake.answer.lock().unwrap() = Answer::Hello;
        answered_ok(&call.join().unwrap());
    });
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "answered after {:?}",
        began.elapsed()
    );
    assert_eq!(w.fake.shown("repo/silent"), 1);
}

#[test]
fn a_request_held_past_the_stream_bound_of_a_drain_is_still_answered() {
    let (wake, mut trigger) = UnixStream::pair().unwrap();
    let mut w = held_served(FRONT_KEY, holding(LONG), move |listener, gate| {
        std::thread::spawn(move || {
            moor_gate::serve_until(listener, &gate, None, &wake, Duration::from_secs(30)).unwrap();
        });
    });
    let session = w.session();
    w.away();
    std::thread::scope(|scope| {
        let w = &w;
        let call = scope.spawn(|| w.status_of(session.as_deref(), "repo/draining"));
        std::thread::sleep(Duration::from_millis(500));
        trigger.write_all(b"x").unwrap();
        std::thread::sleep(
            Duration::from_secs(moor_gate::STREAM_DRAIN_SECS) + Duration::from_secs(1),
        );
        let up = w.up_again();
        answered_ok(&call.join().unwrap());
        up.stop();
    });
    assert_eq!(w.fake.shown("repo/draining"), 1);
}
