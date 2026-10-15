use moor_core::config::parse_user;
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
