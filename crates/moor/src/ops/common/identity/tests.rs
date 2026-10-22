//! Tests for the git identity script and its report.

use std::path::Path;
use std::sync::Arc;

use moor_core::boxscript::quote;
use moor_core::seam::proc::Tool;

use super::Identity;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

fn cx(dir: &Path, name: Option<&str>, email: Option<&str>) -> Cx {
    let proc = Arc::new(ScriptedProc::new());
    for (key, value) in [("user.name", name), ("user.email", email)] {
        let reply = value.map_or_else(
            || Reply::Fail {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
            |v| Reply::Ok(format!("{v}\n")),
        );
        proc.on(Tool::Git, &["config", "--get", key], reply);
    }
    Cx::fake(
        dir,
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        proc,
    )
}

#[test]
fn the_script_sets_name_then_email_and_prints_each_exit() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path(), Some("Ann Dev"), Some("a@x.io"));
    let script = Identity::of(&cx, d.path()).unwrap().script();
    assert_eq!(
        script,
        "git config --global user.name 'Ann Dev'; echo user.name=$?; \
         git config --global user.email 'a@x.io'; echo user.email=$?; "
    );
}

#[test]
fn only_the_known_key_is_set() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path(), None, Some("a@x.io"));
    let script = Identity::of(&cx, d.path()).unwrap().script();
    assert!(script.contains("user.email 'a@x.io'"), "{script}");
    assert!(!script.contains("user.name"), "{script}");
}

#[test]
fn no_author_is_no_identity() {
    let d = tempfile::tempdir().unwrap();
    assert!(Identity::of(&cx(d.path(), None, None), d.path()).is_none());
}

#[test]
fn not_a_repo_is_no_identity() {
    let d = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["config", "--get"],
        Reply::Fail {
            code: 128,
            stdout: String::new(),
            stderr: "fatal: not a git repository".to_owned(),
        },
    );
    let cx = Cx::fake(
        d.path(),
        d.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        proc,
    );
    assert!(Identity::of(&cx, d.path()).is_none());
}

#[test]
fn values_are_shell_quoted_and_carry_no_config_files() {
    let d = tempfile::tempdir().unwrap();
    let hostile = "$(touch x); `id` it's";
    let cx = cx(d.path(), Some(hostile), Some("a@x.io"));
    let script = Identity::of(&cx, d.path()).unwrap().script();
    assert!(script.contains(&format!("user.name {}", quote(hostile))));
    assert!(!script.contains(".gitconfig") && !script.contains("hosts.yml"));
}

#[test]
fn a_failing_key_warns_alone() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path(), Some("Ann Dev"), Some("a@x.io"));
    let identity = Identity::of(&cx, d.path()).unwrap();
    identity.report(&cx, "synced 3 files\nuser.name=3\nuser.email=0\n");
    let warnings = cx.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = warnings.first().unwrap();
    assert_eq!(w.code, "identity_failed");
    assert_eq!(w.message, "could not set git user.name in the box: exit 3");
}

#[test]
fn set_or_unreported_keys_do_not_warn() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path(), Some("Ann Dev"), Some("a@x.io"));
    let identity = Identity::of(&cx, d.path()).unwrap();
    identity.report(&cx, "user.name=0\r\n");
    identity.report(&cx, "");
    assert!(cx.take_warnings().is_empty());
}
