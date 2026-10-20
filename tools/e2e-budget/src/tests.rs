//! Unit tests over recorded event fixtures.

use super::*;

const START_OK: &str = include_str!("../fixtures/start_ok.jsonl");
const START_SLOW_BOX: &str = include_str!("../fixtures/start_slow_box.jsonl");
const START_REAL_RUN: &str = include_str!("../fixtures/start_real_run.jsonl");
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
            "PHASE start.setup took=0.7s budget=none PASS",
            "PHASE start.proxy took=7.0s budget=15.0s PASS",
            "PHASE start.session took=9.5s budget=20.0s PASS",
            "PHASE start.total took=28.8s budget=90.0s PASS",
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
fn real_run_events_yield_a_number_for_every_start_phase() {
    let p = start(START_REAL_RUN);
    assert_eq!(
        lines(&p),
        [
            "PHASE start.box_ready took=37.7s budget=30.0s FAIL",
            "PHASE start.setup took=132.7s budget=none PASS",
            "PHASE start.proxy took=26.3s budget=15.0s FAIL",
            "PHASE start.session took=39.6s budget=20.0s FAIL",
            "PHASE start.total took=103.7s budget=90.0s FAIL",
        ]
    );
    assert!(p.iter().all(|x| x.took_ms.is_some()));
}

#[test]
fn setup_time_is_excluded_from_total_and_never_fails() {
    let events = [
        ev(1, "started", 0),
        ev(2, "box_ready", 10_000),
        ev(3, "setup_finished", 110_000),
        ev(4, "proxy_started", 112_000),
        ev(5, "session_started", 115_000),
    ];
    let p = start_phases(&events, &StartBudgets::default());
    assert_eq!(
        p[1].line(),
        "PHASE start.setup took=100.0s budget=none PASS"
    );
    assert_eq!(p[4].took_ms, Some(15_000));
    assert!(verdict(&p));
}

#[test]
fn start_without_setup_anchors_proxy_on_box_ready() {
    let events = [
        ev(1, "started", 0),
        ev(2, "box_ready", 10_000),
        ev(3, "proxy_started", 12_000),
        ev(4, "session_started", 15_000),
    ];
    let p = start_phases(&events, &StartBudgets::default());
    assert_eq!(p.len(), 4);
    assert_eq!(p[1].took_ms, Some(2_000));
    assert_eq!(p[3].took_ms, Some(15_000));
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
        p[3].line(),
        "PHASE start.session took=missing budget=20.0s FAIL"
    );
    assert_eq!(
        p[4].line(),
        "PHASE start.total took=missing budget=90.0s FAIL"
    );
    assert!(p[..3].iter().all(Phase::pass));
    assert_eq!(
        verdict_line(&p),
        "VERDICT FAIL (2 of 5 phases failed: start.session, start.total)"
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
fn any_skip_is_incomplete_never_pass() {
    let ok = [("a", Outcome::Pass), ("b", Outcome::Skip)];
    assert_eq!(
        summarize(&ok),
        (3, "VERDICT INCOMPLETE (2 phases, 1 skipped: b)".to_owned())
    );
    let bad = [
        ("a", Outcome::Fail),
        ("b", Outcome::Skip),
        ("c", Outcome::Skip),
    ];
    assert_eq!(
        summarize(&bad),
        (
            1,
            "VERDICT FAIL (1 of 3 phases failed: a; 2 skipped: b, c)".to_owned()
        )
    );
    assert_eq!(summarize(&[("a", Outcome::Pass)]).0, 0);
    assert_eq!(summarize(&[]).0, 1);
}

#[test]
fn parses_phase_lines_and_report_lines() {
    assert_eq!(
        parse_phase_line("PHASE wake2 took=0.0s budget=0.0s SKIP"),
        Some(("wake2", Outcome::Skip))
    );
    assert_eq!(parse_phase_line("noise"), None);
    let line = report_line("box_step.1", 2_500);
    assert_eq!(line, "PHASE box_step.1 took=2.5s budget=none PASS");
    assert_eq!(parse_phase_line(&line), Some(("box_step.1", Outcome::Pass)));
}

#[test]
fn parse_jsonl_skips_blank_lines_and_names_bad_line() {
    let ok = parse_jsonl("{\"id\":1,\"kind\":\"started\",\"ts_ms\":5}\n\n").unwrap();
    assert_eq!(ok, [ev(1, "started", 5)]);
    let err = parse_jsonl("{\"id\":1,\"kind\":\"started\",\"ts_ms\":5}\n\nnot json\n").unwrap_err();
    assert!(err.starts_with("line 3:"), "{err}");
}

const PROXY_LOG: &str = "1700000000000 GET gh 200\n1700000005000 POST claude 401\n\
                         1700000009000 CONNECT claude 200\n1700000010000 POST claude 200\n";

#[test]
fn model_request_needs_successful_post_to_the_route() {
    assert!(model_request_after(PROXY_LOG, 1_700_000_000_000, "claude"));
    assert!(!model_request_after("", 0, "claude"));
    assert!(!model_request_after(
        "1700000010000 POST gh 200\n",
        0,
        "claude"
    ));
    assert!(!model_request_after(
        "1700000010000 POST claude 403\n",
        0,
        "claude"
    ));
    assert!(!model_request_after(
        "1700000010000 GET claude 200\n",
        0,
        "claude"
    ));
}

#[test]
fn model_request_before_the_relaunch_does_not_count() {
    assert!(model_request_after(PROXY_LOG, 1_700_000_010_000, "claude"));
    assert!(!model_request_after(PROXY_LOG, 1_700_000_010_001, "claude"));
}

#[test]
fn idle_resumed_agent_posts_only_after_the_steer() {
    // relaunched at ...000, steer at ...005: the only POST is before the steer, so it must not count.
    let log = "1700000001000 POST claude 200\n";
    assert!(model_request_after(log, 1_700_000_000_000, "claude"));
    assert!(!model_request_after(log, 1_700_000_005_000, "claude"));
    let steered = "1700000001000 POST claude 200\n1700000006000 POST claude 200\n";
    assert!(model_request_after(steered, 1_700_000_005_000, "claude"));
    let limited = "1700000001000 POST claude 200\n1700000006000 POST claude 429\n";
    assert!(!model_request_after(limited, 1_700_000_005_000, "claude"));
    assert_eq!(
        model_request_failure(limited, 1_700_000_005_000, "claude"),
        Some("model requests rate limited: 429")
    );
}

#[test]
fn model_request_ignores_malformed_lines() {
    assert!(!model_request_after(
        "POST claude 200\nx POST claude 200\n",
        0,
        "claude"
    ));
    assert!(!model_request_after(
        "1 POST claude 200 extra\n",
        0,
        "claude"
    ));
}

#[test]
fn rate_limited_post_fails_with_a_plain_reason() {
    let log = "1700000010000 POST claude 429\n";
    assert!(!model_request_after(log, 0, "claude"));
    assert_eq!(
        model_request_failure(log, 0, "claude"),
        Some("model requests rate limited: 429")
    );
    assert_eq!(
        model_request_failure(log, 1_700_000_010_001, "claude"),
        None
    );
    assert_eq!(model_request_failure(PROXY_LOG, 0, "claude"), None);
}

#[test]
fn agent_session_is_set_by_either_the_registered_launch_or_the_session() {
    // After start: launch unregistered, the hook already set the session's id.
    assert!(agent_session_set(None, Some("abc")));
    assert!(agent_session_set(Some(""), Some("abc")));
    assert!(agent_session_set(Some("abc"), None));
    assert!(!agent_session_set(None, None));
    assert!(!agent_session_set(Some(""), Some("")));
}
