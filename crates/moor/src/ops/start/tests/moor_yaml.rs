//! Tests for refusing a Box setup when the worktree `.moor.yaml` differs from the base.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::{MoorYamlSource, RepoEntry};
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::Tool;
use moor_core::{BoxPath, ErrorKind, StartReq, TaskId, TaskName};

use super::setup::{is_setup_line, start_req, warning_codes};
use super::{world_at, World};
use crate::cx::Cx;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;

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
fn an_unknown_toolchain_in_the_worktree_fails_before_a_replacement_box_is_acquired() {
    let w = failed_with(Some("setup: [a]\nbox: {toolchains: [nope]}\n"));
    let acquires = || w.driver.calls().iter().filter(|c| *c == "acquire").count();
    let before = acquires();
    let trusted = StartReq {
        trust_moor_yaml: true,
        ..start_req()
    };
    let err = run(&w.cx, &trusted).unwrap_err();
    let msg = err.error.to_string();
    assert!(
        msg.starts_with(".moor.yaml:box.toolchains[0]:")
            && msg.contains("unknown toolchain \"nope\""),
        "{msg}"
    );
    assert_eq!(acquires(), before);
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

const OVERLAY: &str = "setup: [from-overlay]\n";

/// A world whose repo is the controller cache for `demo`, whose entry carries `OVERLAY`.
fn overlay_world(driver: FakeDriver, repo_file: Option<&str>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repos").join("demo");
    std::fs::create_dir_all(&repo).unwrap();
    if let Some(text) = repo_file {
        std::fs::write(repo.join(".moor.yaml"), text).unwrap();
    }
    let proc = Arc::new(ScriptedProc::new());
    let ok = |v: String| Reply::Ok(v);
    proc.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        ok(format!("{}\n", repo.display())),
    );
    proc.on(Tool::Git, &["ls-tree"], ok(String::new()));
    proc.on(Tool::Git, &["rev-parse", "HEAD"], ok("abc123\n".into()));
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ok(format!("{}/.git\n", repo.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--git-path", "info/exclude"],
        ok(format!("{}/exclude\n", dir.path().display())),
    );
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=4).fold(driver, |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
    let surface = Arc::new(FakeSurface::new(dir.path()));
    let table = BTreeMap::from([(
        "demo".to_owned(),
        RepoEntry {
            url: "https://example.test/demo.git".to_owned(),
            default_branch: None,
            moor_yaml: Some(MoorYamlSource::Inline(OVERLAY.to_owned())),
        },
    )]);
    let cx = Cx::fake_with(
        dir.path(),
        &repo,
        driver.clone(),
        surface.clone(),
        proc.clone(),
        crate::testing::cx::fake_config(),
    )
    .with_repos(table);
    World {
        cx,
        driver,
        surface,
        proc,
        dir,
    }
}

/// Whether some setup script run in the box mentions `needle`.
fn setup_mentions(w: &World, needle: &str) -> bool {
    w.driver
        .execs()
        .iter()
        .filter(|e| is_setup_line(&e.1.argv))
        .any(|e| e.1.argv.iter().any(|a| a.contains(needle)))
}

#[test]
fn an_overlay_supplies_setup_without_trust_when_the_repo_has_no_moor_yaml() {
    let w = overlay_world(FakeDriver::new(), None);
    script_base_yaml(&w, None);
    let v = run(&w.cx, &start_req()).unwrap();
    assert!(v.setup.ran && v.setup.ok);
    assert!(setup_mentions(&w, "from-overlay"));
}

#[test]
fn a_worktree_moor_yaml_added_over_an_overlay_is_refused_without_trust() {
    let driver = FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    });
    let w = overlay_world(driver, None);
    script_base_yaml(&w, None);
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let name = TaskName::parse("demo").unwrap();
    let row = w.cx.store().unwrap().open_task_by_name(&name).unwrap();
    let wt = row.unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), "setup: [curl evil]\n").unwrap();
    drop(warning_codes(&w));
    script_base_yaml(&w, None);
    assert_refused(&w, &start_req(), "differs");
    script_base_yaml(&w, None);
    let trusted = StartReq {
        trust_moor_yaml: true,
        ..start_req()
    };
    let again = run(&w.cx, &trusted).unwrap();
    assert!(again.setup.ran && again.setup.ok);
    assert!(setup_mentions(&w, "curl evil"));
    assert!(warning_codes(&w).contains(&"moor_yaml_trusted".to_owned()));
}

#[test]
fn a_repo_moor_yaml_wins_over_the_overlay() {
    let w = overlay_world(FakeDriver::new(), Some(BASE));
    script_base_yaml(&w, Some(BASE));
    let v = run(&w.cx, &start_req()).unwrap();
    assert!(v.setup.ran && v.setup.ok);
    assert!(setup_mentions(&w, "a"));
    assert!(!setup_mentions(&w, "from-overlay"));
}
