use moor_core::config::parse_user;
use moor_core::TaskId;

use super::{req, world_with};
use crate::testing::driver::FakeDriver;

/// Starts `demo` on `provider`; returns the stored egress and the warning codes.
fn start_on(provider: &str) -> (Option<String>, Vec<String>) {
    let config = parse_user("config.yaml", &format!("box: {{provider: {provider}}}")).unwrap();
    let repo = tempfile::tempdir().unwrap();
    let w = world_with(FakeDriver::new(), repo.path(), config);
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let egress = row.unwrap().unwrap().boxed.unwrap().egress;
    let codes = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    (egress, codes)
}

#[test]
fn a_provider_without_a_policy_stores_open_egress() {
    let (egress, codes) = start_on("other");
    assert_eq!(egress.as_deref(), Some("none"));
    assert!(!codes.contains(&"egress_unenforced".to_owned()));
}

#[test]
fn an_applied_allow_list_stores_enforced() {
    let (egress, codes) = start_on("sprites");
    assert_eq!(egress.as_deref(), Some("enforced"));
    assert!(!codes.contains(&"egress_unenforced".to_owned()));
}

#[test]
fn namespace_devbox_without_the_opt_in_stores_not_enforced_and_warns() {
    let (egress, codes) = start_on("namespace-devbox");
    assert_eq!(egress.as_deref(), Some("not_enforced"));
    assert!(codes.contains(&"egress_unenforced".to_owned()));
}
