use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use moor_core::seam::engine::LaunchReq;
use moor_core::{BoxHandle, BoxPath, TaskId, TaskName};

use super::*;
use crate::testing::driver::FakeDriver;

fn req() -> LaunchReq {
    LaunchReq {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/r"),
        workspace: moor_core::seam::surface::WorkspaceRef::new("w"),
        task: TaskName::parse("42").unwrap(),
        task_id: TaskId::new(1),
        branch: "moor/x".to_owned(),
        goal: None,
        resume: None,
        issue: None,
        setup_exit: None,
        model: None,
        nonce: "abc".to_owned(),
        proxy: None,
        origin: None,
    }
}

fn cred() -> BoxPath {
    BoxPath::under("/Users/box/.moor-cred-42-abc", "udid").unwrap()
}

/// A kind for `os` whose `provision` yields `id` and counts its calls.
struct Fake {
    os: &'static str,
    id: Option<&'static str>,
    calls: AtomicUsize,
}

impl DeviceKind for Fake {
    fn available(&self, os: &str) -> bool {
        os == self.os
    }
    fn provision(&self, _: &dyn Driver, _: &LaunchReq, _: &BoxPath) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.id.map(str::to_owned).ok_or_else(|| "no id".to_owned())
    }
    fn env(&self, _: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    fn grants(&self, _: &str, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn brief(&self, _: &str) -> String {
        String::new()
    }
    fn proof(&self, _: &str) -> String {
        String::new()
    }
    fn teardown(&self, _: &str) -> Option<String> {
        None
    }
    fn release_line(&self, _: &str) -> String {
        String::new()
    }
}

const fn fake(os: &'static str, id: Option<&'static str>) -> Fake {
    Fake {
        os,
        id,
        calls: AtomicUsize::new(0),
    }
}

fn run(kinds: &[&'static dyn DeviceKind], os: Option<&str>) -> Vec<String> {
    provision(kinds, &FakeDriver::default(), &req(), &cred(), os)
        .0
        .into_iter()
        .map(|d| d.id)
        .collect()
}

#[test]
fn a_failed_kind_is_skipped_and_the_other_still_provisions() {
    static FAILS: Fake = fake("Darwin", None);
    static WORKS: Fake = fake("Darwin", Some("b"));
    assert_eq!(run(&[&FAILS, &WORKS], Some("Darwin")), ["b"]);
    assert_eq!(FAILS.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_kind_unavailable_for_the_os_is_never_provisioned() {
    static LINUX_ONLY: Fake = fake("Linux", Some("x"));
    static MAC: Fake = fake("Darwin", Some("m"));
    assert_eq!(run(&[&LINUX_ONLY, &MAC], Some("Darwin")), ["m"]);
    assert_eq!(LINUX_ONLY.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unknown_os_provisions_nothing() {
    static MAC: Fake = fake("Darwin", Some("m"));
    assert!(run(&[&MAC], None).is_empty());
    assert_eq!(MAC.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn devices_keep_the_order_of_the_kinds() {
    static A: Fake = fake("Darwin", Some("a"));
    static B: Fake = fake("Darwin", Some("b"));
    static C: Fake = fake("Darwin", Some("c"));
    assert_eq!(run(&[&A, &B, &C], Some("Darwin")), ["a", "b", "c"]);
    assert_eq!(run(&[&C, &A], Some("Darwin")), ["c", "a"]);
}
