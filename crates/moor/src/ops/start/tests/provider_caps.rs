use moor_core::config::parse_user;
use moor_core::seam::driver::Platform;
use moor_core::TaskId;

use super::{req, world_with};
use crate::testing::driver::FakeDriver;

#[test]
fn the_box_egress_is_left_unrecorded_and_never_warned_about() {
    let config = parse_user("config.yaml", "box: {provider: p}").unwrap();
    let repo = tempfile::tempdir().unwrap();
    let w = world_with(FakeDriver::new(), repo.path(), config);
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let egress = row.unwrap().unwrap().boxed.unwrap().egress;
    assert_eq!(egress, None);
    assert!(!w
        .cx
        .take_warnings()
        .iter()
        .any(|x| x.code == "egress_unenforced"));
}

#[test]
fn a_provider_without_reverse_tunnel_is_refused_before_any_acquire() {
    let config = parse_user("config.yaml", "box: {provider: p}").unwrap();
    let repo = tempfile::tempdir().unwrap();
    let w = world_with(
        FakeDriver::new().without_reverse_tunnel_cap(),
        repo.path(),
        config,
    );
    let e = crate::ops::start::run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(e.kind, moor_core::ErrorKind::Refused);
    let msg = e.error.to_string();
    assert!(
        msg.contains("provider p") && msg.contains("reverse_tunnel"),
        "{msg}"
    );
    assert!(e.hints.iter().any(|h| h.contains("reverse tunnel support")));
    assert!(!w.driver.calls().iter().any(|c| c == "acquire"));
}

fn launch_strip_env(config_yaml: &str) -> Vec<String> {
    let config = parse_user("config.yaml", config_yaml).unwrap();
    let repo = tempfile::tempdir().unwrap();
    let w = world_with(FakeDriver::new(), repo.path(), config);
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let row = row.unwrap().unwrap();
    super::super::launch::launch_req(&w.cx, &row, None)
        .unwrap()
        .strip_env
}

#[test]
fn the_box_providers_strip_env_reaches_the_launch_request() {
    let yaml = "box: {provider: external}\ndrivers: {external: {strip_env: [PLATFORM_TOKEN]}}";
    assert_eq!(launch_strip_env(yaml), ["PLATFORM_TOKEN"]);
}

#[test]
fn another_providers_strip_env_is_not_applied() {
    let yaml = "box: {provider: p}\ndrivers: {external: {strip_env: [PLATFORM_TOKEN]}}";
    assert!(launch_strip_env(yaml).is_empty());
}

/// Starts `demo` in a repo whose `.moor.yaml` asks for the xcode capability, on a Box of `platform`.
fn start_with_capabilities(platform: Platform) -> Result<(), crate::error::CommandError> {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        "box: {capabilities: [xcode]}",
    )
    .unwrap();
    let w = world_with(
        FakeDriver::new(),
        repo.path(),
        parse_user("c.yaml", "").unwrap(),
    );
    w.driver.set_platform(platform);
    crate::ops::start::run(&w.cx, &req("demo")).map(drop)
}

#[test]
fn capabilities_on_a_linux_box_fail_the_start_as_a_config_error() {
    let e = start_with_capabilities(Platform::Linux).unwrap_err();
    let msg = e.error.to_string();
    assert!(
        msg.starts_with(
            ".moor.yaml:box.capabilities: box.capabilities applies only to a macOS Box"
        ),
        "{msg}"
    );
}

#[test]
fn capabilities_on_a_macos_box_start_normally() {
    start_with_capabilities(Platform::Macos).unwrap();
}

/// Starts `demo` in a repo that selects toolchain `name`, on a Box of `platform`.
fn start_with_toolchain(name: &str, platform: Platform) -> Result<(), crate::error::CommandError> {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        format!("box: {{toolchains: [{name}]}}"),
    )
    .unwrap();
    let host = "toolchains: {mise: {root: /Users/me/.local/share/mise}}";
    let w = world_with(
        FakeDriver::new(),
        repo.path(),
        parse_user("c.yaml", host).unwrap(),
    );
    w.driver.set_platform(platform);
    crate::ops::start::run(&w.cx, &req("demo")).map(drop)
}

#[test]
fn an_unknown_toolchain_on_a_macos_box_fails_the_start_as_a_config_error() {
    let e = start_with_toolchain("nope", Platform::Macos).unwrap_err();
    let msg = e.error.to_string();
    assert!(
        msg.starts_with(".moor.yaml:box.toolchains[0]:")
            && msg.contains("unknown toolchain \"nope\""),
        "{msg}"
    );
}

#[test]
fn a_known_toolchain_on_a_macos_box_starts_normally() {
    start_with_toolchain("mise", Platform::Macos).unwrap();
}
