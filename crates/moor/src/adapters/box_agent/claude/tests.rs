use moor_core::activity::{HookKind, HookRecord};
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::clock::Ts;

use super::Claude;

fn parse(raw: &str) -> Option<HookRecord> {
    Claude.parse_hook(raw.as_bytes())
}

fn kind(raw: &str) -> Option<HookKind> {
    parse(raw).map(|r| r.kind)
}

#[test]
fn working_state_is_busy() {
    assert_eq!(
        kind(r#"{"state":"working","event":"UserPromptSubmit","ts":1760000000}"#),
        Some(HookKind::Busy)
    );
}

#[test]
fn idle_state_is_idle() {
    assert_eq!(kind(r#"{"state":"idle","ts":1}"#), Some(HookKind::Idle));
}

#[test]
fn starting_state_is_starting() {
    assert_eq!(
        kind(r#"{"state":"starting","ts":1}"#),
        Some(HookKind::Starting)
    );
}

#[test]
fn waiting_input_carries_the_message() {
    assert_eq!(
        kind(r#"{"state":"waiting_input","message":"Allow rm?","ts":1}"#),
        Some(HookKind::Blocked {
            waiting_for: "Allow rm?".into()
        })
    );
}

#[test]
fn waiting_input_without_message_has_empty_waiting_for() {
    assert_eq!(
        kind(r#"{"state":"waiting_input","ts":1}"#),
        Some(HookKind::Blocked {
            waiting_for: String::new()
        })
    );
}

#[test]
fn session_id_is_kept_when_a_string() {
    let rec = parse(r#"{"state":"idle","session_id":"s-1","ts":1}"#).unwrap();
    assert_eq!(rec.session_id.as_deref(), Some("s-1"));
    let rec = parse(r#"{"state":"idle","ts":1}"#).unwrap();
    assert_eq!(rec.session_id, None);
}

#[test]
fn idle_prompt_notification_is_idle_not_blocked() {
    assert_eq!(
        kind(r#"{"state":"waiting_input","notification_type":"idle_prompt","message":"x","ts":1}"#),
        Some(HookKind::Idle)
    );
}

#[test]
fn timestamp_seconds_become_milliseconds() {
    let rec = parse(r#"{"state":"working","ts":1760000000}"#).unwrap();
    assert_eq!(rec.at, Ts::from_millis(1_760_000_000_000));
}

#[test]
fn invalid_json_is_absent() {
    assert_eq!(parse("not json"), None);
    assert_eq!(Claude.parse_hook(&[0xff, 0xfe]), None);
}

#[test]
fn truncated_json_is_absent() {
    assert_eq!(parse(r#"{"state":"working","ts":17600"#), None);
}

#[test]
fn non_object_json_is_absent() {
    assert_eq!(parse("[1,2]"), None);
}

#[test]
fn missing_state_is_absent() {
    assert_eq!(parse(r#"{"ts":1}"#), None);
}

#[test]
fn unknown_state_is_absent() {
    assert_eq!(parse(r#"{"state":"napping","ts":1}"#), None);
}

#[test]
fn missing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle"}"#), None);
}

#[test]
fn string_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":"1760000000"}"#), None);
}

#[test]
fn negative_or_overflowing_ts_is_absent() {
    assert_eq!(parse(r#"{"state":"idle","ts":-1}"#), None);
    assert_eq!(parse(r#"{"state":"idle","ts":9223372036854775807}"#), None);
}
