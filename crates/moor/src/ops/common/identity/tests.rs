//! Tests for the git identity script and its report.

use std::path::Path;
use std::sync::Arc;

use moor_core::boxscript::quote;
use moor_core::seam::forge::GitAuthor;

use super::Identity;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

fn cx(dir: &Path) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
}

fn author(name: &str, email: &str) -> GitAuthor {
    GitAuthor {
        name: name.to_owned(),
        email: email.to_owned(),
    }
}

#[test]
fn the_script_sets_name_then_email_and_prints_each_exit() {
    let script = Identity::of(&author("Ann Dev", "a@x.io")).unwrap().script();
    assert_eq!(
        script,
        "git config --global user.name 'Ann Dev'; echo user.name=$?; \
         git config --global user.email 'a@x.io'; echo user.email=$?; "
    );
}

#[test]
fn only_the_known_key_is_set() {
    let script = Identity::of(&author("", "a@x.io")).unwrap().script();
    assert!(script.contains("user.email 'a@x.io'"), "{script}");
    assert!(!script.contains("user.name"), "{script}");
}

#[test]
fn no_author_is_no_identity() {
    assert!(Identity::of(&author("", "")).is_none());
}

#[test]
fn values_are_shell_quoted_and_carry_no_config_files() {
    let hostile = "$(touch x); `id` it's";
    let script = Identity::of(&author(hostile, "a@x.io")).unwrap().script();
    assert!(script.contains(&format!("user.name {}", quote(hostile))));
    assert!(!script.contains(".gitconfig") && !script.contains("hosts.yml"));
}

#[test]
fn a_failing_key_warns_alone() {
    let d = tempfile::tempdir().unwrap();
    let cx = cx(d.path());
    let identity = Identity::of(&author("Ann Dev", "a@x.io")).unwrap();
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
    let cx = cx(d.path());
    let identity = Identity::of(&author("Ann Dev", "a@x.io")).unwrap();
    identity.report(&cx, "user.name=0\r\n");
    identity.report(&cx, "");
    assert!(cx.take_warnings().is_empty());
}
