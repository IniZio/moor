//! Unit tests over recorded event fixtures.

use super::*;

const START_OK: &str = include_str!("../fixtures/start_ok.jsonl");
const START_SLOW_BOX: &str = include_str!("../fixtures/start_slow_box.jsonl");
const START_NO_SESSION: &str = include_str!("../fixtures/start_no_session.jsonl");
const WAKE_OK: &str = include_str!("../fixtures/wake_ok.jsonl");
const WAKE_SLOW: &str = include_str!("../fixtures/wake_slow_relaunch.jsonl");

fn start(text: &str) -> Vec<Phase> {
    start_phases(&parse_jsonl(text).unwrap(), &StartBudgets::default())
}

fn wake(text: &str) -> Vec<Phase> {
    wake_phases(
        &parse_jsonl(text).unwrap(),
        1_700_000_100_500,
        1,
        &WakeBudgets::default(),
    )
}

fn lines(phases: &[Phase]) -> Vec<String> {
    phases.iter().map(Phase::line).collect()
}

fn ev(id: i64, kind: &str, ts_ms: i64) -> Event {
    Event {
        id,
        kind: kind.into(),
        ts_ms,
    }
}

#[test]
fn start_within_budget_passes_with_exact_lines() {
    let p = start(START_OK);
    assert_eq!(
        lines(&p),
        [
            "PHASE start.box_ready took=12.3s budget=30.0s PASS",
            "PHASE start.proxy took=7.0s budget=15.0s PASS",
            "PHASE start.session took=9.5s budget=20.0s PASS",
            "PHASE start.total took=29.5s budget=90.0s PASS",
        ]
    );
    assert!(verdict(&p));
}

#[test]
fn slow_box_ready_fails_that_phase_only() {
    let p = start(START_SLOW_BOX);
    assert_eq!(
        p[0].line(),
        "PHASE start.box_ready took=31.0s budget=30.0s FAIL"
    );
    assert!(p[1..].iter().all(Phase::pass));
    assert!(!verdict(&p));
}

#[test]
fn exactly_at_budget_passes_one_ms_over_fails() {
    assert!(timed("x", 30_000, 30_000).pass());
    assert!(!timed("x", 30_001, 30_000).pass());
}

#[test]
fn missing_session_started_fails_session_and_total() {
    let p = start(START_NO_SESSION);
    assert_eq!(
        p[2].line(),
        "PHASE start.session took=missing budget=20.0s FAIL"
    );
    assert_eq!(
        p[3].line(),
        "PHASE start.total took=missing budget=90.0s FAIL"
    );
    assert!(p[0].pass() && p[1].pass());
    assert_eq!(
        verdict_line(&p),
        "VERDICT FAIL (2 of 4 phases failed: start.session, start.total)"
    );
}

#[test]
fn endpoint_must_come_after_anchor_by_id() {
    let events = [
        ev(1, "box_ready", 5_000),
        ev(2, "started", 6_000),
        ev(3, "box_ready", 9_000),
    ];
    let p = start_phases(&events, &StartBudgets::default());
    assert_eq!(p[0].took_ms, Some(3_000));
}

#[test]
fn wake_within_budget_passes() {
    let p = wake(WAKE_OK);
    assert_eq!(
        lines(&p),
        [
            "PHASE wake1.woke took=9.5s budget=20.0s PASS",
            "PHASE wake1.relaunch took=10.0s budget=25.0s PASS",
            "PHASE wake1.total took=19.5s budget=45.0s PASS",
        ]
    );
}

#[test]
fn wake_slow_relaunch_fails_relaunch_and_total() {
    let p = wake(WAKE_SLOW);
    assert!(p[0].pass());
    assert_eq!(
        p[1].line(),
        "PHASE wake1.relaunch took=30.0s budget=25.0s FAIL"
    );
    assert_eq!(
        p[2].line(),
        "PHASE wake1.total took=39.5s budget=45.0s PASS"
    );
    assert!(!verdict(&p));
}

#[test]
fn wake_without_woke_is_missing() {
    let events = [ev(1, "relaunched", 9_000)];
    let p = wake_phases(&events, 1_000, 2, &WakeBudgets::default());
    assert_eq!(
        p[0].line(),
        "PHASE wake2.woke took=missing budget=20.0s FAIL"
    );
    assert_eq!(p[1].took_ms, None);
    assert_eq!(p[2].took_ms, None);
}

#[test]
fn counts_tunnel_restarts() {
    let events = parse_jsonl(WAKE_OK).unwrap();
    assert_eq!(count_kind(&events, "tunnel_restarted"), 1);
    assert_eq!(count_kind(&events, "closed"), 1);
    assert_eq!(count_kind(&events, "started"), 0);
}

#[test]
fn verdict_states() {
    let ok = [timed("a", 1, 2), timed("b", 2, 2)];
    assert!(verdict(&ok));
    assert_eq!(verdict_line(&ok), "VERDICT PASS (2 phases)");
    let bad = [timed("a", 3, 2), timed("b", 1, 2), timed("c", 9, 2)];
    assert!(!verdict(&bad));
    assert_eq!(
        verdict_line(&bad),
        "VERDICT FAIL (2 of 3 phases failed: a, c)"
    );
    assert!(!verdict(&[]));
}

#[test]
fn parse_jsonl_skips_blank_lines_and_names_bad_line() {
    let ok = parse_jsonl("{\"id\":1,\"kind\":\"started\",\"ts_ms\":5}\n\n").unwrap();
    assert_eq!(ok, [ev(1, "started", 5)]);
    let err = parse_jsonl("{\"id\":1,\"kind\":\"started\",\"ts_ms\":5}\n\nnot json\n").unwrap_err();
    assert!(err.starts_with("line 3:"), "{err}");
}
