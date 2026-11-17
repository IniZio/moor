#![allow(clippy::unwrap_used, reason = "tests")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Work;

#[tokio::test(start_paused = true)]
async fn a_restart_held_past_its_patience_names_what_holds_it_once_and_still_waits() {
    static WORK: Work = Work::new();
    let forward = WORK.busy("a stream");
    forward.name("listen");
    let boot = WORK.busy("boot of /home/a/moor-1");
    let said = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::clone(&said);
    let restart = tokio::spawn(async move {
        let stuck = move |held: &[String]| told.lock().unwrap().push(held.to_vec());
        WORK.when_idle(Duration::from_mins(30), stuck, || "restarted")
            .await
    });
    tokio::time::sleep(Duration::from_secs(1799)).await;
    assert!(
        said.lock().unwrap().is_empty(),
        "told before its patience ran out"
    );
    drop(boot);
    tokio::time::sleep(Duration::from_hours(2)).await;
    assert_eq!(*said.lock().unwrap(), vec![vec!["listen".to_owned()]]);
    assert!(!restart.is_finished(), "restarted while a forward was open");
    drop(forward);
    assert_eq!(restart.await.unwrap(), "restarted");
}

#[tokio::test(start_paused = true)]
async fn a_stream_that_starts_as_the_last_one_ends_delays_the_restart() {
    static WORK: Work = Work::new();
    let first = WORK.busy("a stream");
    let restart = tokio::spawn(WORK.when_idle(Duration::from_mins(30), |_| {}, || "restarted"));
    tokio::task::yield_now().await;
    drop(first);
    let second = WORK.busy("a stream");
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!restart.is_finished(), "restarted under a stream");
    drop(second);
    assert_eq!(restart.await.unwrap(), "restarted");
}

#[tokio::test]
async fn a_stream_that_starts_while_the_restart_runs_waits_for_it() {
    static WORK: Work = Work::new();
    let started = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&started);
    let mut arriving = None;
    let seen_during = WORK
        .when_idle(
            Duration::from_mins(30),
            |_| {},
            || {
                arriving = Some(std::thread::spawn(move || {
                    let stream = WORK.busy("a stream");
                    seen.store(true, Ordering::SeqCst);
                    drop(stream);
                }));
                std::thread::sleep(Duration::from_millis(200));
                started.load(Ordering::SeqCst)
            },
        )
        .await;
    assert!(
        !seen_during,
        "a stream started between the check and the restart"
    );
    arriving.unwrap().join().unwrap();
}
