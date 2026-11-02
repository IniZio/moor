#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]

use serde_json::json;

use super::{judge, Frame};

/// A hello from machine 7 on `version`, as an agent of this build sends it.
fn hello(version: &str) -> String {
    json!({"hello": {"machine_id": 7, "version": version}}).to_string()
}

fn update_to(version: &str) -> Result<(i64, String), Option<Frame>> {
    Err(Some(Frame::UpdateRequired {
        version: version.to_owned(),
    }))
}

fn judged(stamped: Option<&str>, text: &str) -> Result<(i64, String), Option<Frame>> {
    judge(stamped, text).map(|(machine_id, version, _)| (machine_id, version))
}

#[test]
fn a_stamped_runner_sends_an_older_agent_to_its_own_release() {
    assert_eq!(judged(Some("1.4.0"), &hello("1.3.9")), update_to("1.4.0"));
}

#[test]
fn a_stamped_runner_sends_a_newer_agent_to_its_own_release() {
    assert_eq!(judged(Some("1.4.0"), &hello("1.5.0")), update_to("1.4.0"));
}

#[test]
fn a_stamped_runner_lets_in_an_agent_on_its_release() {
    assert_eq!(
        judged(Some("1.4.0"), &hello("1.4.0")),
        Ok((7, "1.4.0".to_owned()))
    );
}

#[test]
fn an_unstamped_runner_lets_in_any_release() {
    for version in ["0.0.1", "99.0.0", "test"] {
        assert_eq!(judged(None, &hello(version)), Ok((7, version.to_owned())));
    }
}

#[test]
fn a_hello_that_still_names_a_protocol_is_judged_by_its_release() {
    let old = |version: &str| {
        json!({"hello": {"machine_id": 7, "protocol": 1, "version": version}}).to_string()
    };
    assert_eq!(judged(Some("1.4.0"), &old("1.3.0")), update_to("1.4.0"));
    assert_eq!(
        judged(Some("1.4.0"), &old("1.4.0")),
        Ok((7, "1.4.0".to_owned()))
    );
    assert_eq!(judged(None, &old("1.3.0")), Ok((7, "1.3.0".to_owned())));
}

#[test]
fn an_unreadable_hello_is_sent_to_the_runners_release() {
    assert_eq!(judged(Some("1.4.0"), "{\"hi\":1}"), update_to("1.4.0"));
    assert_eq!(
        judged(None, "not json"),
        update_to(env!("CARGO_PKG_VERSION"))
    );
}
