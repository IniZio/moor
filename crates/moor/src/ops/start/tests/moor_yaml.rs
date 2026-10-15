//! Tests for refusing a setup re-run when the worktree `.moor.yaml` differs from the base.

use std::path::Path;

use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, StartReq, TaskId};

use super::setup::{exit, is_setup_line, start_req, start_with, warning_codes};
use super::World;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

/// Scripts the base commit's `.moor.yaml` as `yaml`, or as absent when `None`.
pub(super) fn script_base_yaml(w: &World, yaml: Option<&str>) {
    match yaml {
        Some(text) => {
            w.proc.before(
                Tool::Git,
                &["ls-tree"],
                Reply::Ok(".moor.yaml\n".to_owned()),
            );
            w.proc
                .before(Tool::Git, &["show"], Reply::Ok(text.to_owned()));
        }
        None => {
            w.proc
                .before(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
        }
    }
}

/// Starts `demo` on a base holding `base`, then leaves `worktree_yaml` in its worktree.
///
/// `None` removes the worktree's `.moor.yaml`.
pub(super) fn failed_setup_on_base(
    base: &str,
    worktree_yaml: Option<&str>,
    driver: FakeDriver,
) -> (World, TaskId) {
    let (w, v) = start_with(base, driver);
    let id = TaskId::new(v.task.id);
    let wt = w.cx.store().unwrap().task(id).unwrap().unwrap().worktree;
    let file = Path::new(&wt.unwrap()).join(".moor.yaml");
    match worktree_yaml {
        Some(text) => std::fs::write(file, text).unwrap(),
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
    (w, id)
}

const BASE: &str = "setup: [a]\nenv:\n  A: \"1\"\n";

fn setup_execs(w: &World) -> usize {
    w.driver
        .execs()
        .iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .count()
}

/// A failed first setup, with `worktree_yaml` in the worktree over a `BASE` base.
fn failed_with(worktree_yaml: Option<&str>) -> World {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, _) = failed_setup_on_base(BASE, worktree_yaml, driver);
    drop(warning_codes(&w));
    w
}

/// Asserts the re-run was refused naming `key`, and ran no setup.
fn assert_refused(w: &World, req: &StartReq, key: &str) {
    let err = run(&w.cx, req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains(key), "{}", err.error);
    assert!(err.hints.iter().any(|h| h.contains("--trust-moor-yaml")));
    assert_eq!(setup_execs(w), 1);
}

#[test]
fn an_unchanged_moor_yaml_re_runs_setup() {
    let w = failed_with(Some(BASE));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(setup_execs(&w), 2);
    assert!(!warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn a_changed_setup_is_refused_before_it_runs() {
    let w = failed_with(Some("setup: [a, curl evil]\nenv:\n  A: \"1\"\n"));
    assert_refused(&w, &start_req(), "setup");
}

#[test]
fn a_changed_setup_runs_when_trusted_and_warns() {
    let w = failed_with(Some("setup: [a, curl evil]\nenv:\n  A: \"1\"\n"));
    let trusted = StartReq {
        trust_moor_yaml: true,
        ..start_req()
    };
    let again = run(&w.cx, &trusted).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(setup_execs(&w), 2);
    assert!(warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn a_deleted_moor_yaml_is_refused() {
    let w = failed_with(None);
    assert_refused(&w, &start_req(), "setup");
}

#[test]
fn an_added_moor_yaml_is_refused() {
    let driver = FakeDriver::new().exec_results([Ok(exit(1))]);
    let (w, _) = failed_setup_on_base(BASE, Some(BASE), driver);
    script_base_yaml(&w, None);
    drop(warning_codes(&w));
    assert_refused(&w, &start_req(), "setup");
}

#[test]
fn a_changed_env_is_refused() {
    let w = failed_with(Some("setup: [a]\nenv:\n  A: \"2\"\n"));
    assert_refused(&w, &start_req(), "env");
}
