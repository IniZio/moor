//! Warm starts across commits with real git and real scripts: each Box is a directory, a checkpoint
//! a copy of it, and the repo a local origin, so these assert what a Box's disk really holds.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::seam::driver::ExecOutcome;
use moor_core::view::StartView;
use moor_core::{BoxId, BoxReq, StartReq};

use super::warm::{events, row};
use super::{agentless_req, repo, world, World};
use crate::ops::common::provision::warm_disk_note;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

/// An origin with `.gitignore` of `dist/`, at `<dir>/o/n.git`, and a world whose Boxes clone it.
struct Scratch {
    w: World,
    repo: PathBuf,
    _dir: tempfile::TempDir,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

impl Scratch {
    fn new() -> Self {
        Self::on(FakeDriver::new())
    }

    fn on(driver: FakeDriver) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("o/n.git");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "uploadpack.allowFilter", "true"]);
        git(&repo, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        std::fs::write(repo.join(".gitignore"), "dist/\n").unwrap();
        std::fs::write(repo.join("src"), "one\n").unwrap();
        std::fs::write(repo.join("other"), "x\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        let remote = format!("file://{}/", dir.path().display());
        let driver = driver
            .with_checkpoints()
            .real_trees(&dir.path().join("boxes"), &remote);
        Self {
            w: world(driver),
            repo,
            _dir: dir,
        }
    }

    /// Commits `text` to `path` in the origin; the new commit.
    fn commit(&self, path: &str, text: &str) -> String {
        std::fs::write(self.repo.join(path), text).unwrap();
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-qm", path]);
        self.head()
    }

    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    /// Starts Box `name` at the origin's HEAD with `.moor.yaml` text `yaml`, with no agent.
    fn start(&self, name: &str, yaml: &str) -> (BoxId, StartView) {
        let sha = self.head();
        let at = repo_at(&sha, Some(yaml));
        self.w.forge.answer(repo(), Ok(at));
        let req = StartReq {
            repo: Some(repo()),
            ..agentless_req(name)
        };
        let v = run(&self.w.cx, &req).unwrap();
        (BoxId::new(v.boxed.id), v)
    }

    fn handle(&self, id: BoxId) -> String {
        let row = row(&self.w, id);
        row.boxed.unwrap().handle.unwrap().as_str().to_owned()
    }

    fn tree(&self, id: BoxId) -> PathBuf {
        self.w.driver.tree_of(&self.handle(id)).unwrap()
    }

    fn home(&self, id: BoxId) -> PathBuf {
        self.w.driver.home_of(&self.handle(id)).unwrap()
    }

    fn file(&self, id: BoxId, rel: &str) -> String {
        std::fs::read_to_string(self.tree(id).join(rel)).unwrap()
    }

    fn warm(&self, id: BoxId) -> usize {
        events(&self.w, id, "warm_started").len()
    }
}

const SEEN: &str = "setup: ['cp src \"$HOME/seen\"']\n";

const GUARD: &str = "setup: ['[ -f dist/out ] || { mkdir -p dist && cp src dist/out; }']\n";

const HASHED: &str = "setup: ['[ \"$(cat dist/sum 2>/dev/null)\" = \"$(cksum src)\" ] || { mkdir -p                       dist && cp src dist/out && cksum src > dist/sum; }']\n";

#[test]
fn a_box_at_a_new_commit_starts_warm_on_that_commit_with_a_clean_tree() {
    let s = Scratch::new();
    let (first, _) = s.start("a", SEEN);
    assert_eq!(s.warm(first), 0);
    let saved = s.w.driver.checkpoint_names();
    let new_commit = s.commit("other", "changed\n");
    let (next, v) = s.start("b", SEEN);
    assert_eq!(s.warm(next), 1);
    assert!(v.setup.ok, "{v:?}");
    let tree = s.tree(next);
    assert_eq!(git(&tree, &["rev-parse", "HEAD"]), new_commit);
    assert_eq!(git(&tree, &["status", "--porcelain"]), "");
    assert_eq!(s.file(next, "other"), "changed\n");
    assert_eq!(
        s.w.driver.checkpoint_names(),
        saved,
        "a rerun saves nothing"
    );
}

#[test]
fn a_line_with_no_guard_sees_the_new_commit_s_files() {
    let s = Scratch::new();
    let (a, _) = s.start("a", SEEN);
    assert_eq!(
        std::fs::read_to_string(s.home(a).join("seen")).unwrap(),
        "one\n"
    );
    s.commit("src", "two\n");
    let (c, _) = s.start("c", SEEN);
    assert_eq!(s.warm(c), 1);
    assert_eq!(
        std::fs::read_to_string(s.home(c).join("seen")).unwrap(),
        "two\n"
    );
}

#[test]
fn the_fork_resets_tracked_files_removes_untracked_ones_and_keeps_ignored_ones() {
    let yaml = "setup: ['ls -A > \"$HOME/saw\"; cp src \"$HOME/seen\"; echo x >> src; echo y >                 loose; mkdir -p dist; echo z > dist/out']\n";
    let s = Scratch::new();
    s.start("a", yaml);
    s.commit("other", "changed\n");
    let (b, _) = s.start("b", yaml);
    assert_eq!(s.warm(b), 1);
    let saw = std::fs::read_to_string(s.home(b).join("saw")).unwrap();
    assert!(
        !saw.lines().any(|l| l == "loose"),
        "untracked file gone: {saw}"
    );
    assert!(saw.lines().any(|l| l == "dist"), "ignored dir kept: {saw}");
    let seen = std::fs::read_to_string(s.home(b).join("seen")).unwrap();
    assert_eq!(seen, "one\n", "tracked file reset");
}

#[test]
fn a_tracked_script_that_starts_failing_warns_tries_cold_and_ends_setup_failed_naming_its_line() {
    let yaml = "setup: ['sh ./check']\n";
    let s = Scratch::new();
    s.commit("check", "exit 0\n");
    s.start("a", yaml);
    s.commit("check", "exit 3\n");
    let w = &s.w;
    let forge_at = repo_at(&s.head(), Some(yaml));
    w.forge.answer(repo(), Ok(forge_at));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("d")
    };
    let v = run(&w.cx, &req).unwrap();
    let d = BoxId::new(v.boxed.id);
    assert_eq!(s.warm(d), 1);
    let failed = events(w, d, "warm_rerun_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["line"], "sh ./check");
    assert!(w
        .cx
        .take_warnings()
        .iter()
        .any(|x| x.code == "warm_rerun_failed"));
    assert_eq!(row(w, d).lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(v.setup.line.as_deref(), Some("sh ./check"));
    assert_eq!(s.warm(d), 1, "the second try forked nothing");
}

#[test]
fn a_content_blind_guard_keeps_the_old_output_and_the_box_says_so() {
    let s = Scratch::new();
    let x = s.head();
    let (a, _) = s.start("a", GUARD);
    assert_eq!(s.file(a, "dist/out"), "one\n");
    s.commit("src", "two\n");
    let (next, v) = s.start("next", GUARD);
    assert_eq!(s.warm(next), 1);
    assert_eq!(
        s.file(next, "dist/out"),
        "one\n",
        "the guard skipped its line"
    );
    let short = x.get(..12).unwrap();
    let note = v.warnings_inline.iter().find(|n| n.contains(short));
    let note = note.expect("the start reply names the disk's commit");
    assert!(note.contains("1 tracked path changed"), "{note}");
    assert!(note.contains("dist/"), "{note}");
    assert!(note.contains("may have skipped rebuilding"), "{note}");
    let store = s.w.cx.store().unwrap();
    assert_eq!(warm_disk_note(store, next).as_deref(), Some(note.as_str()));
}

#[test]
fn a_guard_that_checks_a_hash_of_its_source_rebuilds_on_the_next_commit() {
    let s = Scratch::new();
    let (a, _) = s.start("a", HASHED);
    assert_eq!(s.file(a, "dist/out"), "one\n");
    s.commit("src", "two\n");
    let (next, _) = s.start("next", HASHED);
    assert_eq!(s.warm(next), 1);
    assert_eq!(s.file(next, "dist/out"), "two\n");
}

#[test]
fn the_opt_out_starts_cold_on_the_next_commit_and_holds_the_new_content() {
    let yaml = format!("{GUARD}checkpoint: {{across_commits: false}}\n");
    let s = Scratch::new();
    let (a, _) = s.start("a", &yaml);
    assert_eq!(s.file(a, "dist/out"), "one\n");
    s.commit("src", "two\n");
    let (next, _) = s.start("next", &yaml);
    assert_eq!(s.warm(next), 0);
    assert_eq!(s.file(next, "dist/out"), "two\n");
}

/// [`GUARD`] with a line after it, which gets a key no checkpoint holds.
const GUARD_AND_MORE: &str =
    "setup: ['[ -f dist/out ] || { mkdir -p dist && cp src dist/out; }', 'true']\n";

#[test]
fn retry_on_a_rerun_fork_does_not_save_it_as_the_new_base() {
    let clone_dies = ExecOutcome::Exited { code: 72 };
    let driver = FakeDriver::new().exec_ends_once_when("moor_tree_refresh\n", clone_dies);
    let s = Scratch::on(driver);
    s.start("a", GUARD);
    let saved = s.w.driver.checkpoint_names();
    s.commit("src", "two\n");
    s.w.forge
        .answer(repo(), Ok(repo_at(&s.head(), Some(GUARD_AND_MORE))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("b")
    };
    assert!(run(&s.w.cx, &req).is_err(), "the fork's clone exec dies");
    let retry = BoxReq {
        box_name: crate::testing::box_name("b"),
    };
    let v = crate::ops::retry::run(&s.w.cx, &retry).unwrap();
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(s.w.driver.checkpoint_names(), saved, "nothing saved");
    let named = v
        .warnings_inline
        .iter()
        .any(|n| n.contains("may have skipped"));
    assert!(named, "the retry says what it stands on: {v:?}");
    let (later, _) = s.start("c", GUARD_AND_MORE);
    assert_eq!(s.warm(later), 1);
    assert_eq!(events(&s.w, later, "warm_rerun").len(), 1, "no silent skip");
}

#[test]
fn a_line_that_moves_head_saves_no_checkpoint_of_the_tree_it_left() {
    let yaml = "setup: ['git -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q \
                --allow-empty -m moved', 'true']\n";
    let s = Scratch::new();
    let (a, _) = s.start("a", yaml);
    assert_ne!(git(&s.tree(a), &["rev-parse", "HEAD"]), s.head());
    assert_eq!(s.w.driver.checkpoint_names().len(), 1, "the clone alone");
    let warned = s.w.cx.take_warnings();
    assert!(
        warned.iter().any(|x| x.code == "checkpoint_skipped"),
        "{warned:?}"
    );
}

/// A line that leaves a file git neither tracks nor ignores.
const LOOSE: &str = "setup: ['echo made > loose']\n";

#[test]
fn a_same_commit_fork_with_a_kept_tree_skips_the_line_and_loses_what_it_left_untracked() {
    let s = Scratch::on(FakeDriver::new().cache_volumes());
    let (a, _) = s.start("a", LOOSE);
    assert!(s.tree(a).join("loose").exists());
    let (next, v) = s.start("next", LOOSE);
    assert_eq!(s.warm(next), 1);
    assert!(v.setup.ok, "{v:?}");
    assert!(!s.tree(next).join("loose").exists(), "the output is gone");
    assert_eq!(events(&s.w, next, "setup_line_started").len(), 0);
}

#[test]
fn a_same_commit_fork_with_no_kept_tree_keeps_what_the_line_left_untracked_and_runs_it_again() {
    let s = Scratch::new();
    s.start("a", LOOSE);
    let (next, _) = s.start("next", LOOSE);
    assert_eq!(s.warm(next), 1);
    assert_eq!(s.file(next, "loose"), "made\n");
    assert_eq!(events(&s.w, next, "setup_line_started").len(), 1);
}
