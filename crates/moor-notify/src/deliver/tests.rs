use std::cell::RefCell;
use std::time::Duration;

use super::{with_retries, Answer, Endpoint, Retry};
use crate::guard::Reach;

/// Three attempts, 1s then 2s apart, capped at 3s.
const RETRY: Retry = Retry {
    attempts: 3,
    first: Duration::from_secs(1),
    max: Duration::from_secs(3),
};

/// Runs [`with_retries`] over `answers` and returns its result with the waits it slept.
fn run(answers: Vec<Answer>, retry: Retry) -> (Result<u32, String>, Vec<Duration>) {
    let slept = RefCell::new(Vec::new());
    let mut answers = answers.into_iter();
    let got = with_retries(retry, || answers.next().unwrap_or(Answer::Taken), &|d| {
        slept.borrow_mut().push(d);
    });
    (got, slept.into_inner())
}

#[test]
fn statuses_map_to_answers() {
    assert_eq!(Answer::of_status(200), Answer::Taken);
    assert_eq!(Answer::of_status(204), Answer::Taken);
    for retried in [408, 429, 500, 502, 503] {
        assert!(
            matches!(Answer::of_status(retried), Answer::Retry(_)),
            "{retried}"
        );
    }
    for refused in [301, 400, 401, 403, 404, 410] {
        assert!(
            matches!(Answer::of_status(refused), Answer::Refused(_)),
            "{refused}"
        );
    }
}

#[test]
fn a_taken_post_is_not_sent_again() {
    assert_eq!(run(vec![Answer::Taken], RETRY), (Ok(1), vec![]));
}

#[test]
fn a_failure_is_retried_with_doubling_waits() {
    let answers = vec![
        Answer::Retry("HTTP 503".into()),
        Answer::Retry("timeout".into()),
    ];
    assert_eq!(
        run(answers, RETRY),
        (Ok(3), vec![Duration::from_secs(1), Duration::from_secs(2)])
    );
}

#[test]
fn waits_stop_growing_at_the_cap() {
    let retry = Retry {
        attempts: 5,
        ..RETRY
    };
    let answers = vec![Answer::Retry("x".into()); 4];
    let (_, slept) = run(answers, retry);
    assert_eq!(slept, [1, 2, 3, 3].map(Duration::from_secs));
}

#[test]
fn gives_up_after_the_last_attempt() {
    let answers = vec![Answer::Retry("HTTP 503".into()); 3];
    let (got, slept) = run(answers, RETRY);
    assert_eq!(got.unwrap_err(), "HTTP 503 after 3 attempts");
    assert_eq!(slept.len(), 2);
}

#[test]
fn a_refusal_is_not_retried() {
    let (got, slept) = run(vec![Answer::of_status(404)], RETRY);
    assert_eq!(got.unwrap_err(), "HTTP 404, not retried");
    assert!(slept.is_empty());
}

#[test]
fn the_logged_url_drops_path_query_and_user() {
    let endpoint = Endpoint::new(
        "https://me:pw@hooks.example.com:8443/t/0ken?k=v".to_owned(),
        b"k".to_vec(),
        Reach::Public,
    );
    assert_eq!(endpoint.shown(), "https://hooks.example.com:8443");
}
