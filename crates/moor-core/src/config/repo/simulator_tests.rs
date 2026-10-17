//! `box.simulator` parse, validation and platform check.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_repo, RepoConfig, RepoSimulator};

fn ok(text: &str) -> RepoConfig {
    parse_repo("m.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// Asserts the document is rejected with `path` and a message containing `needle`.
fn rejects(text: &str, path: &str, needle: &str) {
    match parse_repo("m.yaml", text) {
        Ok(_) => panic!("accepted: {text}"),
        Err(e) => {
            assert_eq!(e.path, path, "{text}: {e}");
            assert!(e.to_string().contains(needle), "{text}: {e}");
        }
    }
}

#[test]
fn simulator_pin_parses_by_name_or_identifier() {
    let c = ok(
        "box:\n  capabilities: [ios-simulator]\n  simulator: {device_type: \"iPhone 17\", runtime: \"com.apple.CoreSimulator.SimRuntime.iOS-26-4\"}\n",
    );
    assert_eq!(
        c.box_.simulator,
        Some(RepoSimulator {
            device_type: Some("iPhone 17".into()),
            runtime: Some("com.apple.CoreSimulator.SimRuntime.iOS-26-4".into()),
        })
    );
}

#[test]
fn simulator_defaults_to_unset_and_allows_one_field() {
    assert_eq!(ok("box: {size: small}").box_.simulator, None);
    let c = ok("box: {capabilities: [ios-simulator], simulator: {runtime: \"iOS 26.4\"}}");
    let sim = c.box_.simulator.expect("simulator");
    assert_eq!(sim.device_type, None);
    assert_eq!(sim.runtime.as_deref(), Some("iOS 26.4"));
}

#[test]
fn unknown_simulator_key_fails() {
    assert!(parse_repo(
        "m.yaml",
        "box: {capabilities: [ios-simulator], simulator: {os: x}}"
    )
    .is_err());
}

#[test]
fn blank_simulator_values_are_refused() {
    for key in ["device_type", "runtime"] {
        for bad in ["\"\"", "\"  \""] {
            rejects(
                &format!("box: {{capabilities: [ios-simulator], simulator: {{{key}: {bad}}}}}"),
                &format!("box.simulator.{key}"),
                "empty",
            );
        }
    }
}

#[test]
fn simulator_needs_the_capability() {
    rejects(
        "box: {simulator: {device_type: \"iPhone 17\"}}",
        "box.simulator",
        "ios-simulator capability",
    );
    rejects(
        "box: {capabilities: [xcode], simulator: {}}",
        "box.simulator",
        "ios-simulator capability",
    );
}

#[test]
fn simulator_check_platform_both_ways() {
    let c = ok("box: {capabilities: [ios-simulator], simulator: {device_type: \"iPhone 17\"}}");
    assert!(c.box_.check_platform(true).is_ok());
    let mut only_sim = c.box_;
    only_sim.capabilities.clear();
    let e = only_sim.check_platform(false).expect_err("not macOS");
    assert_eq!(e.path, "box.simulator");
    assert!(e.to_string().contains("applies only to a macOS Box"), "{e}");
}
