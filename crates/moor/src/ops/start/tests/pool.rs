//! A provider that names a host, `cloudhypervisor@hv1`, starts a Box only on a machine of the
//! requester that offers that provider and is online; none of it falls back to another provider.

use moor_core::config::{parse_user, EnrolledMachine};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::view::StartView;
use moor_core::ErrorKind;
use moor_core::StartReq;

use super::{req, world_with, World};
use crate::ops::start;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn world_on(provider: &str) -> World {
    let config = parse_user("config.yaml", &format!("box: {{provider: '{provider}'}}")).unwrap();
    world_with(FakeDriver::new(), config)
}

fn enrolled(w: &World, pool: Option<&str>, online: Option<bool>) {
    enrolled_by(w, REQUESTER_USER, pool, online);
}

fn enrolled_by(w: &World, user: UserId, pool: Option<&str>, online: Option<bool>) {
    let store = w.cx.store().unwrap();
    let mut m = EnrolledMachine::parse("hv1", KEY, &[], 2).unwrap();
    if let Some(p) = pool {
        m = m.with_pool(p).unwrap();
    }
    let machine_id = store.enroll_machine(user, &m).unwrap();
    if let Some(online) = online {
        let link = MachineLink {
            machine_id,
            online,
            since: Ts::from_millis(1),
            version: None,
            protocol: None,
        };
        store.set_machine_links(&[link]).unwrap();
    }
}

fn refusal(w: &World) -> String {
    let e = start::run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert_eq!(w.driver.calls(), Vec::<String>::new(), "no driver call");
    e.error.to_string()
}

#[test]
fn a_host_that_is_not_one_of_the_requesters_machines_is_refused() {
    let w = world_on("cloudhypervisor@hv1");
    let msg = refusal(&w);
    assert!(
        msg.contains("hv1") && msg.contains("not one of your machines"),
        "{msg}"
    );
}

#[test]
fn a_host_without_a_pool_is_refused() {
    let w = world_on("cloudhypervisor@hv1");
    enrolled(&w, None, Some(true));
    let msg = refusal(&w);
    assert!(msg.contains("hv1") && msg.contains("no Box pool"), "{msg}");
}

#[test]
fn a_host_whose_pool_is_another_provider_is_refused() {
    let w = world_on("cloudhypervisor@hv1");
    enrolled(&w, Some("firecracker"), Some(true));
    let msg = refusal(&w);
    assert!(
        msg.contains("hv1") && msg.contains("firecracker") && msg.contains("cloudhypervisor"),
        "{msg}"
    );
}

#[test]
fn an_offline_or_never_linked_host_is_refused() {
    for online in [Some(false), None] {
        let w = world_on("cloudhypervisor@hv1");
        enrolled(&w, Some("cloudhypervisor"), online);
        let msg = refusal(&w);
        assert!(
            msg.contains("hv1") && msg.contains("offline"),
            "{online:?}: {msg}"
        );
    }
}

#[test]
fn a_usable_host_goes_on_to_acquire() {
    let w = world_on("cloudhypervisor@hv1");
    enrolled(&w, Some("cloudhypervisor"), Some(true));
    let _ = start::run(&w.cx, &req("demo"));
    let calls = w.driver.calls();
    assert!(calls.iter().any(|c| c.starts_with("acquire")), "{calls:?}");
}

#[test]
fn a_malformed_host_qualified_provider_is_refused() {
    let w = world_on("a@b@c");
    let msg = refusal(&w);
    assert!(msg.contains("a@b@c"), "{msg}");
}

fn held(v: &StartView) -> &str {
    v.boxed.driver.as_ref().map_or("", |d| d.provider.as_str())
}

fn naming(provider: &str) -> StartReq {
    StartReq {
        provider: Some(provider.to_owned()),
        ..req("demo")
    }
}

#[test]
fn a_request_naming_the_requesters_linked_host_replaces_the_configured_provider() {
    let w = world_on("shared");
    enrolled(&w, Some("cloudhypervisor"), Some(true));
    let v = start::run(&w.cx, &naming("cloudhypervisor@hv1")).unwrap();
    assert_eq!(held(&v), "cloudhypervisor@hv1");
    let asked: Vec<String> = w
        .driver
        .acquired_providers()
        .iter()
        .map(|p| p.as_str().to_owned())
        .collect();
    assert_eq!(asked, ["cloudhypervisor@hv1"]);
}

#[test]
fn a_request_naming_another_users_host_is_refused() {
    let w = world_on("shared");
    let (other, _) =
        w.cx.store()
            .unwrap()
            .enroll(&Identity::local(2000), None)
            .unwrap();
    enrolled_by(&w, other, Some("cloudhypervisor"), Some(true));
    let e = start::run(&w.cx, &naming("cloudhypervisor@hv1")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert!(
        e.error.to_string().contains("not one of your machines"),
        "{e:?}"
    );
    assert_eq!(w.driver.calls(), Vec::<String>::new(), "no driver call");
}

#[test]
fn a_request_naming_a_provider_with_no_machine_is_refused() {
    let w = world_on("shared");
    enrolled(&w, Some("cloudhypervisor"), Some(true));
    for provider in ["cloudhypervisor", "a@b@c"] {
        let e = start::run(&w.cx, &naming(provider)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused, "{provider}: {e:?}");
    }
    let e = start::run(&w.cx, &naming("external")).unwrap_err();
    assert!(
        e.error.to_string().contains("one of your own pool hosts"),
        "{e:?}"
    );
    assert_eq!(w.driver.calls(), Vec::<String>::new(), "no driver call");
}

#[test]
fn a_request_naming_a_host_that_is_offline_or_offers_another_pool_is_refused_without_fallback() {
    let w = world_on("shared");
    enrolled(&w, Some("firecracker"), Some(false));
    let e = start::run(&w.cx, &naming("cloudhypervisor@hv1")).unwrap_err();
    assert!(e.error.to_string().contains("firecracker"), "{e:?}");
    assert_eq!(w.driver.calls(), Vec::<String>::new(), "no driver call");
}

#[test]
fn a_request_naming_no_provider_uses_the_configured_one() {
    let w = world_on("shared");
    enrolled(&w, Some("cloudhypervisor"), Some(true));
    let v = start::run(&w.cx, &req("demo")).unwrap();
    assert_eq!(held(&v), "shared");
}

#[test]
fn a_request_naming_a_full_host_is_refused_rather_than_queued() {
    let config = parse_user("config.yaml", "box: {provider: 'shared', max_boxes: 1}").unwrap();
    let w = world_with(FakeDriver::new(), config);
    enrolled(&w, Some("cloudhypervisor"), Some(true));
    start::run(&w.cx, &naming("cloudhypervisor@hv1")).unwrap();
    let second = StartReq {
        provider: Some("cloudhypervisor@hv1".to_owned()),
        ..req("other")
    };
    let e = start::run(&w.cx, &second).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused, "{e:?}");
    assert!(e.error.to_string().contains("Box limit"), "{e:?}");
    let asked = w.driver.acquired_providers().len();
    assert_eq!(asked, 1, "the second start never reached the driver");
    let store = w.cx.store().unwrap();
    let waiting = store.waiting_boxes().unwrap();
    assert!(waiting.is_empty(), "nothing waits for a slot: {waiting:?}");
    let left = store.open_box_by_name(&second.name).unwrap();
    assert!(left.is_none(), "the refused Box is not left open: {left:?}");
}
