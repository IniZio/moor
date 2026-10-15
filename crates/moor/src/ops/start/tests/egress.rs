use moor_core::config::parse_user;
use moor_core::seam::driver::EgressSupport;
use moor_core::TaskId;

use super::{req, world_with};
use crate::testing::driver::FakeDriver;

/// Starts `demo` on a provider reporting `support`; returns the stored egress and the warning codes.
fn start_on(support: EgressSupport) -> (Option<String>, Vec<String>) {
    let config = parse_user("config.yaml", "box: {provider: p}").unwrap();
    let repo = tempfile::tempdir().unwrap();
    let w = world_with(
        FakeDriver::new().egress_support(support),
        repo.path(),
        config,
    );
    let v = crate::ops::start::run(&w.cx, &req("demo")).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(v.task.id));
    let egress = row.unwrap().unwrap().boxed.unwrap().egress;
    let codes = w.cx.take_warnings().into_iter().map(|x| x.code).collect();
    (egress, codes)
}

#[test]
fn a_provider_without_a_policy_stores_open_egress() {
    let (egress, codes) = start_on(EgressSupport::None);
    assert_eq!(egress.as_deref(), Some("none"));
    assert!(!codes.contains(&"egress_unenforced".to_owned()));
}

#[test]
fn an_applied_allow_list_stores_enforced() {
    let (egress, codes) = start_on(EgressSupport::Enforced);
    assert_eq!(egress.as_deref(), Some("enforced"));
    assert!(!codes.contains(&"egress_unenforced".to_owned()));
}

#[test]
fn a_create_only_provider_without_the_opt_in_stores_not_enforced_and_warns() {
    let (egress, codes) = start_on(EgressSupport::OptIn);
    assert_eq!(egress.as_deref(), Some("not_enforced"));
    assert!(codes.contains(&"egress_unenforced".to_owned()));
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
