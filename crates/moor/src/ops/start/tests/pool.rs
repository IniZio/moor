//! A provider that names a host, `cloudhypervisor@hv1`, starts a Box only on a machine of the
//! requester that offers that provider and is online; none of it falls back to another provider.

use moor_core::config::{parse_user, EnrolledMachine, UserMachine};
use moor_core::request::MachineLink;
use moor_core::seam::clock::Ts;
use moor_core::ErrorKind;

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
    let store = w.cx.store().unwrap();
    let mut m = EnrolledMachine::parse("hv1", KEY, &[], 2).unwrap();
    if let Some(p) = pool {
        m = m.with_pool(p).unwrap();
    }
    let machine_id = store.enroll_machine(REQUESTER_USER, &m).unwrap();
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
fn a_machine_reached_over_ssh_is_refused_as_not_enrolled() {
    let w = world_on("cloudhypervisor@hv1");
    let m = UserMachine::parse("hv1", "me@box:22", &[])
        .and_then(|m| m.with_pool("cloudhypervisor"))
        .unwrap();
    w.cx.store()
        .unwrap()
        .set_user_machine(REQUESTER_USER, &m)
        .unwrap();
    assert!(refusal(&w).contains("not enrolled with moor-machine-agent"));
}

#[test]
fn a_malformed_host_qualified_provider_is_refused() {
    let w = world_on("a@b@c");
    let msg = refusal(&w);
    assert!(msg.contains("a@b@c"), "{msg}");
}
