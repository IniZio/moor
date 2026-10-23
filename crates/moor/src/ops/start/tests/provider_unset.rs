use moor_core::config::parse_user;
use moor_core::{BoxName, CloseReq, ErrorKind};

use super::{req, world_with};
use crate::cx::Cx;
use crate::ops::close::run as close;
use crate::ops::start;
use crate::testing::driver::FakeDriver;

#[test]
fn a_start_with_no_provider_fails_before_any_driver_call_and_names_the_setting() {
    let w = super::world_without_provider(FakeDriver::new());
    let e = start::run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let msg = e.error.to_string();
    assert!(msg.contains("box.provider"), "{msg}");
    assert!(msg.contains("config.yaml"), "{msg}");
    assert_eq!(w.driver.calls(), Vec::<String>::new());
}

#[test]
fn a_box_started_with_a_provider_still_attaches_and_closes_when_the_config_drops_it() {
    let config = parse_user("config.yaml", "box: {provider: p}").unwrap();
    let w = world_with(FakeDriver::new(), config);
    let v = start::run(&w.cx, &req("demo")).unwrap();

    let later = Cx::fake_without_provider(
        w.dir.path(),
        w.driver.clone(),
        w.surface.clone(),
        w.proc.clone(),
    );
    let again = start::run(&later, &req("demo")).unwrap();
    assert_eq!(again.boxed.id, v.boxed.id);

    let closed = close(
        &later,
        &CloseReq {
            target: BoxName::parse_allow_bare("demo").unwrap().into(),
            force: true,
        },
    );
    assert!(closed.is_ok(), "{closed:?}");
    assert!(w.driver.calls().iter().any(|c| c.starts_with("release")));
}

/// A Box whose first acquire failed on provider `a`, and a start of it again under `later`.
fn restarted_after_failed_acquire(later: impl FnOnce(&super::World) -> crate::cx::Cx) -> String {
    let config = parse_user("config.yaml", "box: {provider: a}").unwrap();
    let w = world_with(
        FakeDriver::new().fail_acquire(moor_core::seam::driver::DriverError::Failed {
            summary: "boom".into(),
        }),
        config,
    );
    start::run(&w.cx, &req("demo")).unwrap_err();
    let cx = later(&w);
    let v = start::run(&cx, &req("demo")).unwrap();
    let row = cx
        .store()
        .unwrap()
        .box_row(moor_core::BoxId::new(v.boxed.id));
    row.unwrap()
        .unwrap()
        .boxed
        .unwrap()
        .provider
        .as_str()
        .to_owned()
}

#[test]
fn a_box_provisioned_again_stays_on_its_own_provider_when_the_config_drops_it() {
    let provider = restarted_after_failed_acquire(|w| {
        Cx::fake_without_provider(
            w.dir.path(),
            w.driver.clone(),
            w.surface.clone(),
            w.proc.clone(),
        )
        .with_forge(w.forge.clone())
    });
    assert_eq!(provider, "a");
}

#[test]
fn a_box_provisioned_again_stays_on_its_own_provider_when_the_config_names_another() {
    let provider = restarted_after_failed_acquire(|w| {
        let config = parse_user("config.yaml", "box: {provider: b}").unwrap();
        Cx::fake_with(
            w.dir.path(),
            w.driver.clone(),
            w.surface.clone(),
            w.proc.clone(),
            config,
        )
        .with_forge(w.forge.clone())
    });
    assert_eq!(provider, "a");
}
