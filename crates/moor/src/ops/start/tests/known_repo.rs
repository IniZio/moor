use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moor_core::config::{RepoEntry, RepoTable};
use moor_core::seam::proc::Tool;
use moor_core::{ErrorKind, StartReq};

use super::{req, world, World};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::Reply;

const URL: &str = "https://example.com/acme/app.git";
const REFSPEC: &str = "+refs/heads/develop:refs/remotes/origin/develop";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn entry(url: &str, default_branch: Option<&str>) -> RepoEntry {
    RepoEntry {
        url: url.to_owned(),
        default_branch: default_branch.map(str::to_owned),
        moor_yaml: None,
    }
}

fn table(entries: &[(&str, RepoEntry)]) -> RepoTable {
    entries
        .iter()
        .map(|(n, e)| ((*n).to_owned(), e.clone()))
        .collect()
}

fn known(repos: RepoTable) -> World {
    let w = world(FakeDriver::new());
    w.proc.on(Tool::Git, &["clone"], Reply::Ok(String::new()));
    World {
        cx: w.cx.with_repos(repos),
        driver: w.driver,
        surface: w.surface,
        proc: w.proc,
        dir: w.dir,
    }
}

fn repo_req(key: &str) -> StartReq {
    StartReq {
        repo: Some(key.to_owned()),
        ..req("demo")
    }
}

fn cache(w: &World, name: &str) -> PathBuf {
    w.dir.path().join("repos").join(name)
}

fn git_calls(w: &World) -> Vec<Vec<String>> {
    w.proc
        .calls()
        .into_iter()
        .filter(|c| c.tool == Tool::Git)
        .map(|c| c.args)
        .collect()
}

fn cloned(w: &World) -> Vec<Vec<String>> {
    git_calls(w)
        .into_iter()
        .filter(|a| a.first().map(String::as_str) == Some("clone"))
        .collect()
}

#[test]
fn an_unknown_repo_is_refused_and_lists_the_known_ones() {
    let w = known(table(&[("a", entry(URL, None)), ("b", entry("u/b", None))]));
    let err = run(&w.cx, &repo_req("nope")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("a, b"), "{}", err.error);
    assert!(cloned(&w).is_empty());
}

#[test]
fn an_empty_cache_is_cloned_and_the_box_uses_the_cache_path() {
    let w = known(table(&[("app", entry(URL, None))]));
    let v = run(&w.cx, &repo_req("app")).unwrap();
    let dest = cache(&w, "app");
    assert_eq!(
        cloned(&w),
        [vec![
            "clone".to_owned(),
            "--".to_owned(),
            URL.to_owned(),
            dest.display().to_string()
        ]]
    );
    assert_eq!(v.boxed.repo, dest.display().to_string());
    assert!(!git_calls(&w)
        .iter()
        .any(|a| a == &["rev-parse", "--show-toplevel"]));
}

#[test]
fn a_cache_with_a_checkout_is_not_cloned_again() {
    let w = known(table(&[("app", entry(URL, None))]));
    std::fs::create_dir_all(cache(&w, "app").join(".git")).unwrap();
    let v = run(&w.cx, &repo_req("app")).unwrap();
    assert!(cloned(&w).is_empty());
    assert_eq!(v.boxed.repo, cache(&w, "app").display().to_string());
}

#[test]
fn a_default_branch_is_fetched_and_its_sha_is_the_base() {
    let w = known(table(&[("app", entry(URL, Some("develop")))]));
    std::fs::create_dir_all(cache(&w, "app").join(".git")).unwrap();
    w.proc.on(
        Tool::Git,
        &["fetch", "origin", REFSPEC],
        Reply::Ok(String::new()),
    );
    w.proc.on(
        Tool::Git,
        &["rev-parse", "refs/remotes/origin/develop"],
        Reply::Ok(format!("{SHA}\n")),
    );
    w.proc.on(
        Tool::Git,
        &["rev-parse", "--verify"],
        Reply::Ok(format!("{SHA}\n")),
    );
    let v = run(&w.cx, &repo_req("app")).unwrap();
    assert!(git_calls(&w)
        .iter()
        .any(|a| a == &["fetch", "origin", REFSPEC]));
    assert_eq!(v.boxed.base.as_deref(), Some(SHA));
}

#[test]
fn repo_together_with_worktree_is_a_usage_error() {
    let w = known(table(&[("app", entry(URL, None))]));
    let r = StartReq {
        worktree: Some("/some/worktree".into()),
        ..repo_req("app")
    };
    let err = run(&w.cx, &r).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(cloned(&w).is_empty());
}

#[test]
fn a_repo_is_found_by_url() {
    let w = known(table(&[("app", entry(URL, None))]));
    let v = run(&w.cx, &repo_req("https://example.com/acme/app")).unwrap();
    assert_eq!(v.boxed.repo, cache(&w, "app").display().to_string());
    assert_eq!(cloned(&w).len(), 1);
}

#[test]
fn a_start_waits_for_the_repo_lock_and_then_clones_once() {
    let w = known(table(&[("app", entry(URL, None))]));
    let dest = cache(&w, "app");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let held = std::fs::File::create(dest.with_file_name("app.lock")).unwrap();
    held.lock().unwrap();
    let released = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            released.store(true, Ordering::SeqCst);
            held.unlock().unwrap();
        });
        run(&w.cx, &repo_req("app")).unwrap();
        assert!(released.load(Ordering::SeqCst), "start ran under the lock");
    });
    assert_eq!(cloned(&w).len(), 1);
}
