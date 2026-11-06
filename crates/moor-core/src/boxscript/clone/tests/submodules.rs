//! The submodules a clone leaves, under `sh` with real git; kept trees run the refresh first.

use std::path::{Path, PathBuf};
use std::process::Output;

use super::super::{at_base, clone_script, left_out_submodules, script, Checkout};
use crate::boxscript::refresh::{outcome, refresh_script};
use crate::testgit::{Origin, BRANCH};

fn repo(o: &Origin, name: &str) -> PathBuf {
    let dir = o.path(name);
    o.git(&o.path(""), &["init", "-q", "-b", "main", name]);
    std::fs::write(dir.join("s.txt"), format!("{name}\n")).unwrap();
    o.git(&dir, &["add", "."]);
    o.git(&dir, &["commit", "-q", "-m", "s"]);
    o.git(&dir, &["config", "uploadpack.allowFilter", "true"]);
    o.git(&dir, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    dir
}

fn url_of(dir: &Path) -> String {
    format!("file://{}", dir.display())
}

fn add(o: &Origin, to: &Path, dir: &Path, path: &str) {
    o.git(to, &["submodule", "add", "-q", &url_of(dir), path]);
    o.git(to, &["commit", "-q", "-m", "submodule"]);
}

fn add_link(o: &Origin, path: &str, url: &str) {
    let seed = o.seed();
    std::fs::create_dir_all(seed.join(path)).unwrap();
    let gitmodules = seed.join(".gitmodules");
    let before = std::fs::read_to_string(&gitmodules).unwrap_or_default();
    let entry = format!("[submodule \"{path}\"]\n\tpath = {path}\n\turl = {url}\n");
    std::fs::write(&gitmodules, before + &entry).unwrap();
    o.git(&seed, &["add", ".gitmodules"]);
    let link = format!("160000,{},{path}", "1".repeat(40));
    o.git(&seed, &["update-index", "--add", "--cacheinfo", &link]);
}

fn publish(o: &Origin) -> String {
    o.push("main", &[])
}

fn run(o: &Origin, script: &str, vars: &[(&str, &str)]) -> (Output, String) {
    let out = o.sh(&o.tree(), script, vars);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout.clone()).unwrap();
    (out, text)
}

fn clone(o: &Origin, base: &str) -> String {
    let url = o.url();
    let c = Checkout {
        url: &url,
        base,
        branch: BRANCH,
    };
    run(o, &clone_script(&c), &[]).1
}

fn start(o: &Origin, key: &str, base: &str) -> String {
    let url = o.url();
    let c = Checkout {
        url: &url,
        base,
        branch: BRANCH,
    };
    let both = format!("{}{}", refresh_script(&c, key), clone_script(&c));
    run(o, &both, &[]).1
}

fn read(o: &Origin, rel: &str) -> String {
    std::fs::read_to_string(o.tree().join(rel)).unwrap()
}

fn left_out_paths(text: &str) -> Vec<&str> {
    left_out_submodules(text)
        .into_iter()
        .map(|l| l.split(": ").next().unwrap())
        .collect()
}

/// A `PATH` whose `git` logs to the returned file, and sleeps on arguments matching `hang`.
fn logging_git(o: &Origin, hang: &str) -> (String, PathBuf) {
    let bin = o.path("wrapper");
    std::fs::create_dir_all(&bin).unwrap();
    let log = o.path("git.log");
    let real = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap();
    let wrapper = format!(
        "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in {hang}) sleep 60;; esac\nexec '{}' \"$@\"\n",
        log.display(),
        real.trim()
    );
    let git = bin.join("git");
    std::fs::write(&git, wrapper).unwrap();
    std::fs::set_permissions(&git, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}:{}",
        bin.display(),
        o.home().join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path, log)
}

#[test]
fn a_fresh_clone_leaves_submodules_initialised_nested_too() {
    let o = Origin::new("sub-fresh");
    let inner = repo(&o, "inner");
    let middle = repo(&o, "middle");
    add(&o, &middle, &inner, "deep");
    add(&o, &o.seed(), &middle, "lib");
    let base = publish(&o);

    let text = clone(&o, &base);

    assert_eq!(read(&o, "lib/s.txt"), "middle\n");
    assert_eq!(read(&o, "lib/deep/s.txt"), "inner\n");
    assert!(left_out_submodules(&text).is_empty(), "{text}");
    assert_eq!(
        o.git(&o.tree(), &["symbolic-ref", "--short", "HEAD"]),
        BRANCH
    );
    assert!(
        at_base(&text),
        "a clean tree with submodules is the base: {text}"
    );
}

#[test]
fn a_submodule_of_a_partial_clone_is_partial_too_and_checks_out() {
    let o = Origin::new("sub-partial");
    let sub = repo(&o, "sub");
    add(&o, &o.seed(), &sub, "lib");
    let base = publish(&o);

    clone(&o, &base);

    let lib = o.tree().join("lib");
    assert_eq!(
        o.git(&lib, &["config", "remote.origin.partialclonefilter"]),
        "blob:none"
    );
    assert_eq!(read(&o, "lib/s.txt"), "sub\n");
    assert_eq!(o.git(&lib, &["status", "--porcelain"]), "");
}

#[test]
fn a_broken_submodule_leaves_a_usable_tree_its_siblings_and_a_warning() {
    let o = Origin::new("sub-broken");
    let good = repo(&o, "good");
    add_link(&o, "abroken", &url_of(&o.path("missing")));
    add(&o, &o.seed(), &good, "zgood");
    let base = publish(&o);

    let text = clone(&o, &base);

    assert_eq!(left_out_paths(&text), ["abroken"], "{text}");
    let reason = *left_out_submodules(&text).first().unwrap();
    assert!(reason.contains("fatal:"), "the reason is git's: {reason}");
    assert_eq!(read(&o, "a.txt"), "one\n");
    assert_eq!(read(&o, "zgood/s.txt"), "good\n");
    assert_eq!(
        o.git(&o.tree(), &["symbolic-ref", "--short", "HEAD"]),
        BRANCH
    );
    assert!(
        !at_base(&text),
        "a tree missing a submodule must not be saved as the base: {text}"
    );
}

#[test]
fn a_broken_nested_submodule_is_named_by_its_full_path() {
    let o = Origin::new("sub-broken-nested");
    let middle = repo(&o, "middle");
    let link = format!("160000,{},deep", "1".repeat(40));
    let missing = url_of(&o.path("missing"));
    o.git(
        &middle,
        &["config", "-f", ".gitmodules", "submodule.deep.path", "deep"],
    );
    o.git(
        &middle,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.deep.url",
            &missing,
        ],
    );
    o.git(&middle, &["add", ".gitmodules"]);
    o.git(&middle, &["update-index", "--add", "--cacheinfo", &link]);
    o.git(&middle, &["commit", "-q", "-m", "broken nested"]);
    add(&o, &o.seed(), &middle, "lib");
    let base = publish(&o);

    let text = clone(&o, &base);

    assert_eq!(read(&o, "lib/s.txt"), "middle\n");
    assert_eq!(left_out_paths(&text), ["lib/deep"], "{text}");
}

#[test]
fn a_refused_submodule_does_not_send_the_next_box_cold() {
    let o = Origin::new("sub-kept-refused");
    let good = repo(&o, "good");
    add_link(&o, "refused", &url_of(&o.path("missing")));
    add(&o, &o.seed(), &good, "lib");
    let base = publish(&o);
    let first = start(&o, "moor-1-aaaaaa", &base);
    assert_eq!(left_out_paths(&first), ["refused"], "{first}");
    std::fs::create_dir_all(o.tree().join("build")).unwrap();
    std::fs::write(o.tree().join("build/out"), "kept").unwrap();

    let second = start(&o, "moor-2-bbbbbb", &base);

    assert_eq!(outcome(&second), Some("refreshed"), "{second}");
    assert_eq!(left_out_paths(&second), ["refused"], "{second}");
    assert_eq!(read(&o, "build/out"), "kept");
    assert_eq!(read(&o, "lib/s.txt"), "good\n");
}

#[test]
fn a_kept_tree_without_its_submodule_gets_it_on_the_next_box() {
    let o = Origin::new("sub-kept");
    let base = o.base.clone();
    start(&o, "moor-1-aaaaaa", &base);
    std::fs::create_dir_all(o.tree().join("build")).unwrap();
    std::fs::write(o.tree().join("build/out"), "kept").unwrap();
    let sub = repo(&o, "sub");
    add(&o, &o.seed(), &sub, "lib");
    o.push(BRANCH, &[]);

    let text = start(&o, "moor-2-bbbbbb", &base);

    assert_eq!(outcome(&text), Some("refreshed"), "{text}");
    assert_eq!(read(&o, "lib/s.txt"), "sub\n");
    assert_eq!(read(&o, "build/out"), "kept");
}

#[test]
fn a_rerun_leaves_a_submodule_where_the_session_moved_it() {
    let o = Origin::new("sub-rerun");
    let sub = repo(&o, "sub");
    add(&o, &o.seed(), &sub, "lib");
    let base = publish(&o);
    clone(&o, &base);
    let lib = o.tree().join("lib");
    o.git(
        &lib,
        &["commit", "-q", "--allow-empty", "-m", "session work"],
    );
    let moved = o.git(&lib, &["rev-parse", "HEAD"]);

    clone(&o, &base);

    assert_eq!(o.git(&lib, &["rev-parse", "HEAD"]), moved);
}

#[test]
fn a_repo_with_no_gitmodules_runs_no_submodule_command() {
    let o = Origin::new("sub-none");
    let (path, log) = logging_git(&o, "no-such-args");
    let url = o.url();
    let c = Checkout {
        url: &url,
        base: &o.base,
        branch: BRANCH,
    };

    run(&o, &clone_script(&c), &[("PATH", &path)]);

    let calls = std::fs::read_to_string(log).unwrap();
    assert!(calls.contains("clone"), "the wrapper ran: {calls}");
    assert!(!calls.contains("submodule"), "{calls}");
    assert!(!calls.contains("ls-files"), "{calls}");
}

#[test]
fn a_submodule_past_the_budget_is_left_out_and_the_script_finishes() {
    let o = Origin::new("sub-slow");
    let sub = repo(&o, "sub");
    add(&o, &o.seed(), &sub, "lib");
    let base = publish(&o);
    let (path, _) = logging_git(&o, "*submodule*update*");
    let url = o.url();
    let c = Checkout {
        url: &url,
        base: &base,
        branch: BRANCH,
    };

    let (_, text) = run(&o, &script(&c, 2), &[("PATH", &path)]);

    assert_eq!(left_out_paths(&text), ["lib"], "{text}");
    assert!(text.contains("out of time"), "{text}");
    assert_eq!(read(&o, "a.txt"), "one\n");
}
