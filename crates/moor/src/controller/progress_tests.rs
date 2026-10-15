use std::thread::sleep;
use std::time::Duration;

use super::report_while;
use moor_core::controller::ServerFrame;
use moor_core::Response;

const TICK: Duration = Duration::from_millis(40);

fn done() -> Response {
    Response::ok("start", None)
}

#[test]
fn slow_work_emits_progress_with_the_request_id() {
    let mut frames = Vec::new();
    let response = report_while(
        "start",
        "r7",
        TICK,
        &mut |f| {
            frames.push(f);
            true
        },
        || {
            sleep(TICK * 3 + TICK / 2);
            done()
        },
    );
    assert_eq!(response.cmd, "start");
    assert!(frames.len() >= 2, "got {} frames", frames.len());
    assert!(frames
        .iter()
        .all(|f| matches!(f, ServerFrame::Progress { .. })));
    for frame in &frames {
        let ServerFrame::Progress { id, message } = frame else {
            continue;
        };
        assert_eq!(id, "r7");
        assert!(message.starts_with("start: still running ("), "{message}");
    }
}

#[test]
fn fast_work_emits_nothing() {
    let mut count = 0;
    let response = report_while(
        "wait",
        "r1",
        Duration::from_secs(30),
        &mut |_| {
            count += 1;
            true
        },
        done,
    );
    assert_eq!(response.cmd, "start");
    assert_eq!(count, 0);
}

#[test]
fn a_gone_client_stops_emits_but_work_finishes() {
    let mut count = 0;
    let response = report_while(
        "run",
        "r2",
        TICK,
        &mut |_| {
            count += 1;
            false
        },
        || {
            sleep(TICK * 4 + TICK / 2);
            done()
        },
    );
    assert_eq!(response.cmd, "start");
    assert_eq!(count, 1);
}
