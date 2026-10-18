//! The tail of a passing setup's output, which `moor start` shows.

use moor_core::domain::run::SyncMode;
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::secret::Secret;
use moor_core::{BoxId, BoxPath};

use super::{cfg, rig, run_setup, run_setup_with, SetupRun};
use crate::ops::common::box_exec::BoxProxy;
use crate::testing::driver::FakeDriver;

fn tail_of(run: &SetupRun) -> Vec<String> {
    let view = run.view();
    assert!(view.ok, "{run:?}");
    view.tail
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_passing_setup_returns_the_tail_of_its_log() {
    let d = tempfile::tempdir().unwrap();
    let log = (1..=30).fold(String::new(), |acc, i| acc + &format!("line {i}\n"));
    let r = rig(d.path(), FakeDriver::new().copy_text(log));
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let tail = tail_of(&got);
    assert_eq!(tail.len(), 20);
    assert_eq!(tail.first().map(String::as_str), Some("line 11"));
    assert_eq!(tail.last().map(String::as_str), Some("line 30"));
    assert!(r.cx.take_warnings().is_empty());
}

#[test]
fn a_log_that_cannot_be_copied_leaves_the_tail_empty() {
    let d = tempfile::tempdir().unwrap();
    let r = rig(d.path(), FakeDriver::new());
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    assert!(tail_of(&got).is_empty());
}

#[test]
fn the_tail_drops_control_characters_and_keeps_the_proxy_nonce_out() {
    let d = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let log = "\u{1b}[32mgreen\u{1b}[0m\nHTTPS_PROXY=http://moor:s3cret-value@127.0.0.1:4000\n";
    let r = rig(
        d.path(),
        FakeDriver::new()
            .peek_text("box-1", &environ, "HOME=/home/box\0")
            .copy_text(log),
    );
    let proxy = BoxProxy {
        box_id: BoxId::new(1),
        addr: ProxyAddr {
            box_port: 4000,
            nonce: Secret::new(b"s3cret-value".to_vec()),
            ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".to_owned(),
            forge: false,
            box_env: Vec::new(),
        },
        origin: None,
        rule_env: Vec::new(),
    };
    let got = run_setup_with(
        &r.cx,
        &r.handle,
        "p",
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
        Some(&proxy),
    );
    let tail = tail_of(&got);
    assert_eq!(tail.first().map(String::as_str), Some("green"));
    assert!(!tail.join("\n").contains("s3cret-value"), "{tail:?}");
}
