use moor_core::config::parse_user;
use moor_core::BoxId;

use super::{req, world_with};
use crate::testing::driver::FakeDriver;

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
    let yaml = "nodes: {external: {runtime: external}}\ndrivers: {external: {strip_env: [PLATFORM_TOKEN]}}";
    assert_eq!(launch_strip_env(yaml), ["PLATFORM_TOKEN"]);
}

#[test]
fn another_providers_strip_env_is_not_applied() {
    let yaml = "nodes: {p: {runtime: p}}\ndrivers: {external: {strip_env: [PLATFORM_TOKEN]}}";
    assert!(launch_strip_env(yaml).is_empty());
}
