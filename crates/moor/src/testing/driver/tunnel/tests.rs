//! Self-tests for the tunnel half of the Driver fake.

use std::path::{Path, PathBuf};

use moor_core::name::BoxName;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, TunnelFailure, TunnelHandle};
use moor_core::{BoxHandle, BoxId, ProviderId};

use super::{FakeDriver, TunnelCall, TunnelReply};

fn boxed(fake: &FakeDriver) -> BoxHandle {
    let got = fake.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: BoxId::new(1),
        name: BoxName::parse_allow_bare("t1").unwrap(),
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    });
    got.unwrap().handle
}

fn open(fake: &FakeDriver, h: &BoxHandle) -> Result<Box<dyn TunnelHandle>, DriverError> {
    fake.reverse_tunnel(h, 8080, 9090, Path::new("/host"))
}

fn caps(fake: &FakeDriver) -> bool {
    fake.caps(&ProviderId::new("fake")).unwrap().reverse_tunnel
}

#[test]
fn request_fields_are_recorded() {
    let fake = FakeDriver::new().reverse_tunnels();
    let h = boxed(&fake);
    open(&fake, &h).unwrap();
    assert_eq!(
        fake.tunnels(),
        [TunnelCall {
            handle: h,
            box_port: 8080,
            host_port: 9090,
            host_cwd: PathBuf::from("/host"),
        }]
    );
    assert_eq!(fake.calls(), ["acquire", "reverse_tunnel box-1"]);
}

#[test]
fn requests_are_unsupported_by_default() {
    let fake = FakeDriver::new();
    let h = boxed(&fake);
    assert!(caps(&fake));
    assert_eq!(
        open(&fake, &h).err(),
        Some(DriverError::Unsupported("reverse tunnel"))
    );
    assert_eq!(fake.tunnels().len(), 1);
}

#[test]
fn caps_report_false_when_the_capability_is_withheld() {
    let fake = FakeDriver::new()
        .reverse_tunnels()
        .without_reverse_tunnel_cap();
    assert!(!caps(&fake));
}

#[test]
fn enabled_reports_caps_and_hands_out_live_tunnel() {
    let fake = FakeDriver::new().reverse_tunnels();
    let h = boxed(&fake);
    assert!(caps(&fake));
    let t = open(&fake, &h).unwrap();
    assert!(t.alive());
    assert_eq!(t.pid(), 50_000);
    assert_eq!(open(&fake, &h).unwrap().pid(), 50_001);
}

#[test]
fn exit_after_ready_returns_dead_handle() {
    let fake = FakeDriver::new()
        .reverse_tunnels()
        .tunnel_replies([TunnelReply::ExitAfterReady]);
    let h = boxed(&fake);
    assert!(!open(&fake, &h).unwrap().alive());
    assert!(open(&fake, &h).unwrap().alive());
}

#[test]
fn early_exit_and_hang_fail_with_scripted_errors() {
    let fake = FakeDriver::new().reverse_tunnels().tunnel_replies([
        TunnelReply::EarlyExit {
            code: 255,
            stderr: "denied".to_owned(),
        },
        TunnelReply::Hang,
    ]);
    let h = boxed(&fake);
    assert_eq!(
        open(&fake, &h).err(),
        Some(DriverError::Tunnel {
            failure: TunnelFailure::Exited {
                code: Some(255),
                signal: None
            },
            stderr_tail: "denied".to_owned(),
        })
    );
    assert_eq!(
        open(&fake, &h).err(),
        Some(DriverError::Tunnel {
            failure: TunnelFailure::TimedOut(std::time::Duration::from_secs(30)),
            stderr_tail: String::new(),
        })
    );
}

#[test]
fn end_tunnel_flips_alive() {
    let fake = FakeDriver::new().reverse_tunnels();
    let h = boxed(&fake);
    let t = open(&fake, &h).unwrap();
    fake.end_tunnel(t.pid());
    assert!(!t.alive());
}

#[test]
fn drop_records_pid() {
    let fake = FakeDriver::new().reverse_tunnels();
    let h = boxed(&fake);
    let t = open(&fake, &h).unwrap();
    assert!(fake.dropped_tunnels().is_empty());
    drop(t);
    assert_eq!(fake.dropped_tunnels(), [50_000]);
}

#[test]
fn unknown_handle_is_not_found_but_recorded() {
    let fake = FakeDriver::new().reverse_tunnels();
    let ghost = BoxHandle::new("ghost");
    assert_eq!(open(&fake, &ghost).err(), Some(DriverError::NotFound));
    assert_eq!(fake.tunnels().len(), 1);
}
