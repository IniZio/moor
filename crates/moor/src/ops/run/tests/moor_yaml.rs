//! Tests for refusing a run when the worktree `.moor.yaml` differs from the base.

use moor_core::seam::proc::Tool;
use moor_core::ErrorKind;

use super::stage::command_execs;
use super::{ready, req, Fx};
use crate::ops::run::run;
use crate::testing::proc::Reply;

const BASE: &str = "setup: [a]\n";
const CHANGED: &str = "setup: [a, b]\n";

/// A Ready Box whose base commit holds `base` as its `.moor.yaml`, and whose worktree holds `now`.
fn with_yaml(base: Option<&str>, now: &str) -> Fx {
    let f = ready();
    if let Some(text) = base {
        f.proc
            .before(Tool::Git, &["ls-tree"], Reply::Ok(".moor.yaml\n".into()));
        f.proc.before(Tool::Git, &["show"], Reply::Ok(text.into()));
    }
    std::fs::write(f.dir.path().join(".moor.yaml"), now).unwrap();
    f
}

fn warning_codes(f: &Fx) -> Vec<String> {
    f.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

/// Asserts the run was refused naming `key`, with no exec and no run row.
fn assert_refused(f: &Fx, key: &str) {
    let err = run(&f.cx, &req(&["true"])).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains(key), "{}", err.error);
    assert!(err.hints.iter().any(|h| h.contains("--trust-moor-yaml")));
    assert!(f.driver.execs().is_empty());
    assert!(f.cx.store().unwrap().last_run(f.id).unwrap().is_none());
}

#[test]
fn unchanged_moor_yaml_runs_the_command() {
    let f = with_yaml(Some(BASE), BASE);
    let v = run(&f.cx, &req(&["true"])).unwrap();
    assert!(v.ran);
    assert_eq!(command_execs(&f).len(), 1);
    assert!(!warning_codes(&f).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn changed_setup_is_refused() {
    let f = with_yaml(Some(BASE), CHANGED);
    assert_refused(&f, "setup");
}

#[test]
fn trusting_a_changed_moor_yaml_runs_and_warns_once() {
    let f = with_yaml(Some(BASE), CHANGED);
    let mut r = req(&["true"]);
    r.trust_moor_yaml = true;
    let v = run(&f.cx, &r).unwrap();
    assert!(v.ran);
    assert_eq!(command_execs(&f).len(), 1);
    let trusted = warning_codes(&f)
        .into_iter()
        .filter(|c| c == "moor_yaml_trusted")
        .count();
    assert_eq!(trusted, 1);
}

#[test]
fn moor_yaml_added_to_the_worktree_is_refused() {
    let f = with_yaml(None, CHANGED);
    f.proc
        .before(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    assert_refused(&f, "added");
}
