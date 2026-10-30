#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::cache::CachePlan;
use moor_core::config::UserMachine;
use moor_core::name::BoxName;
use moor_core::seam::driver::{AcquireReq, Driver};
use moor_core::seam::proc::Tool;
use moor_core::{BoxHandle, BoxId, ProviderId};

use super::{forward_args, keep, MachineLink, Via};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{ChildReply, ScriptedProc};

fn machine(ssh: &str) -> UserMachine {
    UserMachine::parse("mini", ssh, &["macos".to_owned()]).expect("valid machine")
}

fn boxed(fake: &FakeDriver) -> BoxHandle {
    fake.acquire(&AcquireReq {
        pool_machine: None,
        provider: ProviderId::new("fake"),
        box_id: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").expect("name"),
        class: None,
        nonce: "n0".to_owned(),
        cache: CachePlan::none(),
    })
    .expect("acquired")
    .handle
}

#[test]
fn ssh_argv_forwards_each_port_to_its_host_tunnel() {
    let args = forward_args(
        &machine("dev@mac.example:2222"),
        &[(8081, 41001), (1337, 41002)],
    );
    assert_eq!(
        args,
        [
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "BatchMode=yes",
            "-R",
            "8081:127.0.0.1:41001",
            "-R",
            "1337:127.0.0.1:41002",
            "-p",
            "2222",
            "--",
            "dev@mac.example"
        ]
    );
}

#[test]
fn ssh_argv_unbrackets_an_ipv6_host() {
    let args = forward_args(&machine("dev@[::1]"), &[(8080, 41000)]);
    assert_eq!(args[args.len() - 2..], ["--", "dev@::1"]);
    assert!(!args.contains(&"-p".to_owned()));
}

#[test]
fn opens_a_tunnel_per_port_then_one_ssh_child() {
    let (driver, proc) = (FakeDriver::new().reverse_tunnels(), ScriptedProc::new());
    proc.on_child(Tool::Ssh, &[], ChildReply::Hang);
    let h = boxed(&driver);
    let mut link = MachineLink::open(
        &driver,
        &proc,
        &h,
        &Via::Ssh(&machine("dev@mac")),
        &[8081, 3001],
    )
    .expect("open");
    assert!(link.alive());
    let ports: Vec<u16> = driver.tunnels().iter().map(|t| t.box_port).collect();
    assert_eq!(ports, [8081, 3001]);
    let ssh: Vec<_> = proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Ssh)
        .collect();
    assert_eq!(ssh.len(), 1);
    assert!(ssh[0].args.contains(&"8081:127.0.0.1:28081".to_owned()));
    assert!(ssh[0].args.contains(&"3001:127.0.0.1:23001".to_owned()));
}

#[test]
fn a_dead_tunnel_is_revived_and_close_ends_every_child() {
    let (driver, proc) = (FakeDriver::new().reverse_tunnels(), ScriptedProc::new());
    proc.on_child(Tool::Ssh, &[], ChildReply::Hang);
    let h = boxed(&driver);
    let open = || MachineLink::open(&driver, &proc, &h, &Via::Ssh(&machine("dev@mac")), &[8081]);
    let mut link = Some(open().expect("open"));
    let first = driver.tunnels().len();
    keep(&mut link, open).expect("alive link kept");
    assert_eq!(driver.tunnels().len(), first, "a live link is left alone");
    driver.end_tunnel(50_000);
    keep(&mut link, open).expect("revived");
    assert_eq!(
        driver.tunnels().len(),
        first + 1,
        "the dead link was opened again"
    );
    assert_eq!(proc.dropped().len(), 1, "the old ssh child was ended");
    drop(link);
    assert_eq!(proc.dropped().len(), 2, "close ends the ssh child");
    assert!(
        driver.dropped_tunnels().contains(&50_001),
        "close ends the tunnel"
    );
}

#[test]
fn a_failed_tunnel_names_its_port() {
    let (driver, proc) = (FakeDriver::new(), ScriptedProc::new());
    let h = boxed(&driver);
    let err = MachineLink::open(&driver, &proc, &h, &Via::Ssh(&machine("dev@mac")), &[8081])
        .err()
        .expect("no tunnel support");
    assert_eq!(err.port, Some(8081));
}
