//! Tests for deriving open limits from events.

use super::*;
use serde_json::{json, Value};

fn limits(events: &[(&str, Value)]) -> Vec<Limit> {
    open_limits(events.iter().map(|(k, v)| (*k, v)))
}

fn limited(rule: &str, resets: Option<&str>) -> (&'static str, Value) {
    (
        "agent_rate_limited",
        json!({"rule": rule, "status": 429, "resets_at": resets}),
    )
}

fn rejected(rule: &str) -> (&'static str, Value) {
    (
        "proxy_credential_rejected",
        json!({"rule": rule, "status": 401}),
    )
}

fn recovered(rule: &str) -> (&'static str, Value) {
    ("proxy_credential_recovered", json!({"rule": rule}))
}

#[test]
fn rate_limit_opens_a_limit_with_plain_message() {
    let l = limits(&[limited("claude", None)]);
    assert_eq!(
        l,
        [Limit {
            kind: LimitKind::RateLimited,
            rule: "claude".into(),
            status: 429,
            resets_at: None,
        }]
    );
    assert_eq!(
        l.iter().map(Limit::message).collect::<Vec<_>>(),
        ["Claude account rate limited (429)"]
    );
}

#[test]
fn reset_time_ends_the_message() {
    let l = limits(&[limited("claude", Some("2026-10-17T21:00:00Z"))]);
    assert_eq!(
        l.iter().map(Limit::message).collect::<Vec<_>>(),
        ["Claude account rate limited (429), resets 2026-10-17T21:00:00Z"]
    );
}

#[test]
fn repeated_rate_limit_stays_one_limit() {
    let l = limits(&[limited("claude", None), limited("claude", None)]);
    assert_eq!(l.len(), 1);
}

#[test]
fn recovery_closes_the_limit() {
    assert!(limits(&[limited("claude", None), recovered("claude")]).is_empty());
}

#[test]
fn rejection_after_recovery_reopens_as_rejected() {
    let l = limits(&[
        limited("claude", None),
        recovered("claude"),
        rejected("claude"),
    ]);
    assert_eq!(
        l.iter()
            .map(|l| (l.kind.as_str(), l.message()))
            .collect::<Vec<_>>(),
        [(
            "proxy_credential_rejected",
            "Claude credential rejected (401)".to_owned()
        )]
    );
}

#[test]
fn rules_are_independent_and_sorted() {
    let l = limits(&[
        limited("zed", None),
        limited("claude", None),
        recovered("zed"),
        limited("alpha", None),
    ]);
    let rules: Vec<_> = l.iter().map(|l| l.rule.as_str()).collect();
    assert_eq!(rules, ["alpha", "claude"]);
}

#[test]
fn unrelated_kinds_are_ignored() {
    let other = ("note", json!({"rule": "claude", "status": 429}));
    assert!(limits(&[other]).is_empty());
}
