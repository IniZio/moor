use moor_core::config::parse_user;
use moor_core::BoxId;

use super::{req, world_with};
use crate::testing::driver::FakeDriver;

#[test]
fn the_box_egress_is_left_unrecorded_and_never_warned_about() {
    let config = parse_user("config.yaml", "box: {provider: p}").unwrap();
    let w = world_with(FakeDriver::new(), config);
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
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
    let w = world_with(FakeDriver::new().without_reverse_tunnel_cap(), config);
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
    let w = world_with(FakeDriver::new(), config);
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().box_row(BoxId::new(v.boxed.id));
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
