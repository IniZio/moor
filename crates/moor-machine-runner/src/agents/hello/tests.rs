#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unnecessary_wraps,
    reason = "tests assert on fixed fixtures"
)]

use serde_json::json;

use super::{judge, Frame, MIN_AGENT_VERSION};

/// The floor these tests judge against, below the runner releases they use.
const FLOOR: &str = "0.3.0-nightly.191";

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
    judge(stamped, FLOOR, text).map(|(machine_id, version, _)| (machine_id, version))
}

fn linked(version: &str) -> Result<(i64, String), Option<Frame>> {
    Ok((7, version.to_owned()))
}

#[test]
fn the_floor_is_a_semantic_version() {
    assert!(semver::Version::parse(MIN_AGENT_VERSION).is_ok());
}

#[test]
fn nightlies_order_by_their_number_and_below_their_release() {
    let v = |s: &str| semver::Version::parse(s).unwrap();
    assert!(v("0.3.0-nightly.191") > v("0.3.0-nightly.190"));
    assert!(v("0.3.0-nightly.191") < v("0.3.0"));
    assert!(v("0.3.0-nightly.10") > v("0.3.0-nightly.9"));
}

#[test]
fn a_newer_runner_links_an_older_agent_at_or_above_the_floor() {
    let runner = Some("0.3.0-nightly.200");
    assert_eq!(judged(runner, &hello(FLOOR)), linked(FLOOR));
    assert_eq!(
        judged(runner, &hello("0.3.0-nightly.199")),
        linked("0.3.0-nightly.199")
    );
}

#[test]
fn an_agent_below_the_floor_is_sent_to_the_runners_release() {
    let runner = Some("0.3.0-nightly.200");
    assert_eq!(
        judged(runner, &hello("0.3.0-nightly.190")),
        update_to("0.3.0-nightly.200")
    );
    assert_eq!(
        judged(runner, &hello("0.2.0")),
        update_to("0.3.0-nightly.200")
    );
}

#[test]
fn a_runner_links_an_agent_newer_than_itself() {
    assert_eq!(
        judged(Some("0.3.0-nightly.200"), &hello("0.3.0")),
        linked("0.3.0")
    );
}

#[test]
fn a_runner_below_the_floor_links_its_own_release_and_newer() {
    let runner = Some("0.2.1");
    assert_eq!(judged(runner, &hello("0.2.1")), linked("0.2.1"));
    assert_eq!(judged(runner, &hello("0.2.2")), linked("0.2.2"));
    assert_eq!(judged(runner, &hello("0.2.0")), update_to("0.2.1"));
}

#[test]
fn an_agent_whose_release_is_not_semver_is_sent_to_the_runners_release() {
    assert_eq!(judged(Some("0.3.0"), &hello("test")), update_to("0.3.0"));
}

#[test]
fn an_unstamped_runner_lets_in_any_release() {
    for version in ["0.0.1", "99.0.0", "test"] {
        assert_eq!(judged(None, &hello(version)), linked(version));
    }
}

#[test]
fn a_hello_that_still_names_a_protocol_is_judged_by_its_release() {
    let old = |version: &str| {
        json!({"hello": {"machine_id": 7, "protocol": 1, "version": version}}).to_string()
    };
    assert_eq!(judged(Some("0.3.0"), &old("0.2.0")), update_to("0.3.0"));
    assert_eq!(judged(Some("0.3.0"), &old("0.3.0")), linked("0.3.0"));
    assert_eq!(judged(None, &old("0.2.0")), linked("0.2.0"));
}

#[test]
fn an_unreadable_hello_is_sent_to_the_runners_release() {
    assert_eq!(judged(Some("0.3.0"), "{\"hi\":1}"), update_to("0.3.0"));
    assert_eq!(
        judged(None, "not json"),
        update_to(env!("CARGO_PKG_VERSION"))
    );
}
