//! Tests that a repo's `box.size` decides the machine class a Box is asked for.

use moor_core::config::UserConfig;

use super::setup::{start_req, start_with};
use super::warm::{after_a_partial_setup_of, start, LINES};
use super::{script_base_yaml, world, world_with};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

fn classes(w: &super::World) -> Vec<String> {
    (w.driver.asked_classes().into_iter())
        .map(|c| c.expect("a class is always asked for"))
        .collect()
}

#[test]
fn a_repo_asking_for_large_gets_the_large_class() {
    let (w, _) = start_with("box:\n  size: large\n", FakeDriver::new());
    assert_eq!(classes(&w), ["large"]);
}

#[test]
fn a_repo_size_beats_the_users() {
    let user: UserConfig =
        moor_core::config::parse_user("config.yaml", "box:\n  size: medium\n").unwrap();
    let w = world_with(FakeDriver::new(), user);
    script_base_yaml(&w, Some("box:\n  size: xlarge\n"));
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(classes(&w), ["xlarge"]);
}

#[test]
fn a_box_with_no_configured_size_asks_for_no_class() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(w.driver.asked_classes(), [None]);
}

#[test]
fn a_user_size_alone_is_asked_for() {
    let user: UserConfig =
        moor_core::config::parse_user("config.yaml", "box:\n  size: medium\n").unwrap();
    let w = world_with(FakeDriver::new(), user);
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    assert_eq!(classes(&w), ["medium"]);
}

#[test]
fn a_warm_fork_with_no_size_asks_for_no_class_either() {
    let (w, _) = after_a_partial_setup_of(FakeDriver::new(), LINES);
    start(&w, "second", "abc123", LINES);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("fork")));
    assert_eq!(w.driver.asked_classes(), [None, None]);
}

#[test]
fn a_warm_fork_takes_the_repo_size_too() {
    let yaml = format!("box:\n  size: large\n{LINES}");
    let (w, _) = after_a_partial_setup_of(FakeDriver::new(), &yaml);
    start(&w, "second", "abc123", &yaml);
    assert!(w.driver.calls().iter().any(|c| c.starts_with("fork")));
    assert_eq!(classes(&w), ["large", "large"]);
}
