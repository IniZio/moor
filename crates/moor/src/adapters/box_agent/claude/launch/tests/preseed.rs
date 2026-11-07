//! The first-run state merged into `~/.claude.json`.

use std::fs;

use moor_core::seam::agent_adapter::AgentAdapter;
use serde_json::json;
use tempfile::TempDir;

use super::super::super::Claude;
use super::{any_interpreter, ctx, forced, read_json, run_step, run_step_on, Interp};

fn check_preseed_creates_and_merges(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let file = format!("{}/.claude.json", c.home.as_str());
    let setup = Claude.preseed(&c).unwrap();
    fs::create_dir_all(c.home.as_str()).unwrap();
    assert!(run_step_on(&setup, path).status.success());
    let wd = c.workdir.as_str();
    let fresh = read_json(&file);
    assert_eq!(fresh["hasCompletedOnboarding"], true);
    assert_eq!(fresh["theme"], "dark");
    assert_eq!(fresh["projects"][wd]["hasTrustDialogAccepted"], true);

    fs::write(
        &file,
        json!({"kept": "u", "projects": {wd: {"allowedTools": ["x"]}, "/o": {}}}).to_string(),
    )
    .unwrap();
    assert!(run_step_on(&setup, path).status.success());
    let merged = read_json(&file);
    assert_eq!(merged["kept"], "u");
    assert_eq!(merged["projects"]["/o"], json!({}));
    assert_eq!(merged["projects"][wd]["allowedTools"], json!(["x"]));
    assert_eq!(merged["projects"][wd]["hasTrustDialogAccepted"], true);
}

#[test]
fn preseed_creates_and_merges_claude_json() {
    if any_interpreter() {
        check_preseed_creates_and_merges(None);
    }
}

#[test]
fn preseed_merges_with_python3_only() {
    forced(Interp::Python, check_preseed_creates_and_merges);
}

#[test]
fn preseed_merges_with_node_only() {
    forced(Interp::Node, check_preseed_creates_and_merges);
}

fn check_preseed_without_interpreter(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let file = format!("{}/.claude.json", c.home.as_str());
    let setup = Claude.preseed(&c).unwrap();
    fs::create_dir_all(c.home.as_str()).unwrap();
    assert!(run_step_on(&setup, path).status.success());
    assert_eq!(
        read_json(&file)["projects"][c.workdir.as_str()]["hasTrustDialogAccepted"],
        true
    );
    assert!(
        !run_step_on(&setup, path).status.success(),
        "a home this step wrote may hold what the agent wrote since"
    );
    let launched = r#"{"keep":1,"moorIdentityFresh":true}"#;
    fs::write(&file, launched).unwrap();
    let out = run_step_on(&setup, path);
    assert!(!out.status.success(), "an agent's state cannot be merged");
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not preseed"));
    assert_eq!(fs::read_to_string(&file).unwrap(), launched);
}

#[test]
fn preseed_without_an_interpreter_writes_fresh_or_fails() {
    forced(Interp::Neither, check_preseed_without_interpreter);
}

/// Claude Code keys folder trust by the physical cwd, so a symlinked workdir needs both keys.
fn check_preseed_trusts_symlinked_workdir(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let real = t.path().join("cache/worktree");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, c.workdir.as_str()).unwrap();
    let file = format!("{}/.claude.json", c.home.as_str());
    fs::create_dir_all(c.home.as_str()).unwrap();
    let setup = Claude.preseed(&c).unwrap();

    assert!(run_step_on(&setup, path).status.success());
    let projects = &read_json(&file)["projects"];
    assert_eq!(projects[c.workdir.as_str()]["hasTrustDialogAccepted"], true);
    assert_eq!(
        projects[real.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
}

#[test]
fn preseed_trusts_the_resolved_workdir_with_python3_only() {
    forced(Interp::Python, check_preseed_trusts_symlinked_workdir);
}

#[test]
fn preseed_trusts_the_resolved_workdir_with_node_only() {
    forced(Interp::Node, check_preseed_trusts_symlinked_workdir);
}

#[test]
fn preseed_trusts_the_resolved_workdir_without_an_interpreter() {
    forced(Interp::Neither, check_preseed_trusts_symlinked_workdir);
}

/// Claude Code takes a trusted repository root for any folder inside it.
fn check_preseed_trusts_the_repository_root(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let repo = t.path().join("cache/repo");
    let real = repo.join("sub/dir");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir_all(repo.join(".git")).unwrap();
    std::os::unix::fs::symlink(&real, c.workdir.as_str()).unwrap();
    let file = format!("{}/.claude.json", c.home.as_str());
    fs::create_dir_all(c.home.as_str()).unwrap();
    let setup = Claude.preseed(&c).unwrap();

    assert!(run_step_on(&setup, path).status.success());
    let projects = &read_json(&file)["projects"];
    for key in [
        c.workdir.as_str(),
        real.to_str().unwrap(),
        repo.to_str().unwrap(),
    ] {
        assert_eq!(
            projects[key]["hasTrustDialogAccepted"], true,
            "{key}: {projects}"
        );
    }
}

#[test]
fn preseed_trusts_the_repository_root_with_python3_only() {
    forced(Interp::Python, check_preseed_trusts_the_repository_root);
}

#[test]
fn preseed_trusts_the_repository_root_with_node_only() {
    forced(Interp::Node, check_preseed_trusts_the_repository_root);
}

#[test]
fn preseed_trusts_the_repository_root_without_an_interpreter() {
    forced(Interp::Neither, check_preseed_trusts_the_repository_root);
}

#[test]
fn preseed_failure_fails_the_step() {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let path = format!("{}/.claude.json", c.home.as_str());
    fs::create_dir_all(c.home.as_str()).unwrap();
    fs::write(&path, "not json").unwrap();
    let out = run_step(&Claude.preseed(&c).unwrap());
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not preseed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
}

/// What a checkpoint holds when a plugin install ran before the agent ever started.
const FORKED: &str = r#"{"machineID":"m","userID":"u","firstStartTime":"t","migrationVersion":14}"#;

fn check_a_forked_home_gets_a_new_identity_once(path: Option<&str>, merges: bool) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let file = format!("{}/.claude.json", c.home.as_str());
    let setup = Claude.preseed(&c).unwrap();
    fs::create_dir_all(c.home.as_str()).unwrap();
    fs::write(&file, FORKED).unwrap();
    assert!(run_step_on(&setup, path).status.success());
    let first = read_json(&file);
    for name in ["machineID", "userID", "firstStartTime"] {
        assert!(first.get(name).is_none(), "{name} stayed: {first}");
    }
    assert_eq!(first["hasCompletedOnboarding"], true);
    if merges {
        assert_eq!(first["migrationVersion"], 14);
        let mut started = first;
        started["userID"] = json!("this-box");
        fs::write(&file, started.to_string()).unwrap();
        assert!(run_step_on(&setup, path).status.success());
        assert_eq!(
            read_json(&file)["userID"],
            "this-box",
            "a relaunch keeps it"
        );
    }
}

#[test]
fn a_forked_home_gets_a_new_identity_once_with_python3_only() {
    forced(Interp::Python, |p| {
        check_a_forked_home_gets_a_new_identity_once(p, true);
    });
}

#[test]
fn a_forked_home_gets_a_new_identity_once_with_node_only() {
    forced(Interp::Node, |p| {
        check_a_forked_home_gets_a_new_identity_once(p, true);
    });
}

#[test]
fn a_forked_home_gets_a_new_identity_without_an_interpreter() {
    forced(Interp::Neither, |p| {
        check_a_forked_home_gets_a_new_identity_once(p, false);
    });
}

fn check_a_fresh_home_keeps_the_identity_its_first_launch_made(path: Option<&str>) {
    let t = TempDir::new().unwrap();
    let c = ctx(t.path());
    let file = format!("{}/.claude.json", c.home.as_str());
    let setup = Claude.preseed(&c).unwrap();
    fs::create_dir_all(c.home.as_str()).unwrap();
    assert!(run_step_on(&setup, path).status.success());
    let mut started = read_json(&file);
    started["userID"] = json!("this-box");
    fs::write(&file, started.to_string()).unwrap();
    assert!(run_step_on(&setup, path).status.success());
    assert_eq!(read_json(&file)["userID"], "this-box");
}

#[test]
fn a_fresh_home_keeps_the_identity_its_first_launch_made() {
    forced(
        Interp::Python,
        check_a_fresh_home_keeps_the_identity_its_first_launch_made,
    );
    forced(
        Interp::Node,
        check_a_fresh_home_keeps_the_identity_its_first_launch_made,
    );
}
