//! Tests for refusing a Box setup when the worktree `.moor.yaml` differs from the base.

use std::path::Path;

use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, StartReq, TaskId, TaskName};

use super::setup::{is_setup_line, start_req, warning_codes};
use super::{world_at, World};
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

/// Starts `demo` on a base holding `base` with the Box acquire failing, then leaves
/// `worktree_yaml` in its worktree.
///
/// The Task is left `SetupFailed` with a lost Box, so the next start provisions a new one.
/// `None` removes the worktree's `.moor.yaml`.
fn lost_box_on_base(base: &str, worktree_yaml: Option<&str>) -> World {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), base).unwrap();
    let driver = FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    });
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(base));
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let name = TaskName::parse("demo").unwrap();
    let row = w.cx.store().unwrap().open_task_by_name(&name).unwrap();
    let wt = row.unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    let file = Path::new(&wt).join(".moor.yaml");
    match worktree_yaml {
        Some(text) => std::fs::write(file, text).unwrap(),
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
    drop(warning_codes(&w));
    w
}

const BASE: &str = "setup: [a]\nenv:\n  A: \"1\"\n";

fn setup_execs(w: &World) -> usize {
    w.driver
        .execs()
        .iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .count()
}

/// A lost Box, with `worktree_yaml` in the worktree over a `BASE` base.
fn failed_with(worktree_yaml: Option<&str>) -> World {
    let w = lost_box_on_base(BASE, worktree_yaml);
    script_base_yaml(&w, Some(BASE));
    w
}

/// Asserts the start was refused naming `key`, and ran no setup.
fn assert_refused(w: &World, req: &StartReq, key: &str) {
    let err = run(&w.cx, req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains(key), "{}", err.error);
    assert!(err.hints.iter().any(|h| h.contains("--trust-moor-yaml")));
    assert_eq!(setup_execs(w), 0);
}

#[test]
fn an_unchanged_moor_yaml_sets_up_a_replacement_box() {
    let w = failed_with(Some(BASE));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(setup_execs(&w), 1);
    assert!(!warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn a_changed_setup_is_refused_before_a_replacement_box_is_set_up() {
    let w = failed_with(Some("setup: [a, curl evil]\nenv:\n  A: \"1\"\n"));
    assert_refused(&w, &start_req(), "setup");
}

#[test]
fn a_changed_setup_sets_up_a_replacement_box_when_trusted_and_warns() {
    let w = failed_with(Some("setup: [a, curl evil]\nenv:\n  A: \"1\"\n"));
    let trusted = StartReq {
        trust_moor_yaml: true,
        ..start_req()
    };
    let again = run(&w.cx, &trusted).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert_eq!(setup_execs(&w), 1);
    assert!(warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn a_deleted_moor_yaml_is_refused_for_a_replacement_box() {
    let w = failed_with(None);
    assert_refused(&w, &start_req(), "deleted");
}

#[test]
fn an_added_moor_yaml_is_refused_for_a_replacement_box() {
    let w = lost_box_on_base(BASE, Some(BASE));
    script_base_yaml(&w, None);
    assert_refused(&w, &start_req(), "added");
}

#[test]
fn a_changed_env_is_refused_for_a_replacement_box() {
    let w = failed_with(Some("setup: [a]\nenv:\n  A: \"2\"\n"));
    assert_refused(&w, &start_req(), "env");
}

#[test]
fn the_base_copy_is_read_by_host_git_in_the_main_checkout() {
    let w = failed_with(Some(BASE));
    run(&w.cx, &start_req()).unwrap();
    let row = w.cx.store().unwrap().task(TaskId::new(1)).unwrap().unwrap();
    let base = row.base.clone().unwrap();
    let wt = std::path::PathBuf::from(row.worktree.unwrap());
    let repo = std::path::PathBuf::from(&row.repo);
    assert_ne!(repo, wt);
    let want = format!("{base}:.moor.yaml");
    let calls = w.proc.calls();
    for sub in ["ls-tree", "show"] {
        let hit: Vec<_> = calls
            .iter()
            .filter(|c| c.tool == Tool::Git && c.args.iter().any(|a| a == sub))
            .filter(|c| c.args.iter().any(|a| a.contains(".moor.yaml")))
            .collect();
        assert!(!hit.is_empty(), "no git {sub} call");
        for c in hit {
            assert_eq!(
                c.cwd.as_deref(),
                Some(repo.as_path()),
                "{sub}: {:?}",
                c.args
            );
        }
    }
    let show = calls
        .iter()
        .find(|c| c.args.iter().any(|a| a == "show"))
        .unwrap();
    assert!(show.args.contains(&want), "{:?}", show.args);
}

#[test]
fn a_replacement_box_starts_when_the_worktree_copy_equals_the_base_but_the_main_checkout_differs() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), BASE).unwrap();
    let driver = FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    });
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(BASE));
    run(&w.cx, &start_req()).unwrap_err();
    let name = TaskName::parse("demo").unwrap();
    let row = w.cx.store().unwrap().open_task_by_name(&name).unwrap();
    let wt = row.unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), BASE).unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), "setup: [evil]\n").unwrap();
    script_base_yaml(&w, Some(BASE));
    let again = run(&w.cx, &start_req()).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert!(!warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}
