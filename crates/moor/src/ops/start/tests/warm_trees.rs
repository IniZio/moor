//! Warm starts across commits with real git and real scripts: each Box is a directory, a checkpoint
//! a copy of it, and the repo a local origin, so these assert what a Box's disk really holds.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::path::{Path, PathBuf};
use std::process::Command;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::driver::ExecOutcome;
use moor_core::view::StartView;
use moor_core::{BoxId, BoxReq, StartReq};

use super::warm::{events, row};
use super::warm_commits::setup_calls_on;
use super::{agentless_req, repo, world, World};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::{resume_setup, warm_disk_note};
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::repo_at;

/// An origin with `.gitignore` of `dist/`, at `<dir>/o/n.git`, and a world whose Boxes clone it.
pub(super) struct Scratch {
    pub(super) w: World,
    pub(super) repo: PathBuf,
    pub(super) dir: tempfile::TempDir,
}

pub(super) fn git(repo: &Path, args: &[&str]) -> String {
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
    pub(super) fn new() -> Self {
        Self::on(FakeDriver::new())
    }

    pub(super) fn on(driver: FakeDriver) -> Self {
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
            dir,
        }
    }

    /// Commits `text` to `path` in the origin; the new commit.
    pub(super) fn commit(&self, path: &str, text: &str) -> String {
        std::fs::write(self.repo.join(path), text).unwrap();
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-qm", path]);
        self.head()
    }

    pub(super) fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    /// Starts Box `name` at the origin's HEAD with `.moor.yaml` text `yaml`, with no agent.
    pub(super) fn start(&self, name: &str, yaml: &str) -> (BoxId, StartView) {
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

    pub(super) fn handle(&self, id: BoxId) -> String {
        let row = row(&self.w, id);
        row.boxed.unwrap().handle.unwrap().as_str().to_owned()
    }

    pub(super) fn tree(&self, id: BoxId) -> PathBuf {
        self.w.driver.tree_of(&self.handle(id)).unwrap()
    }

    pub(super) fn home(&self, id: BoxId) -> PathBuf {
        self.w.driver.home_of(&self.handle(id)).unwrap()
    }

    pub(super) fn file(&self, id: BoxId, rel: &str) -> String {
        std::fs::read_to_string(self.tree(id).join(rel)).unwrap()
    }

    /// Pushes a commit of `text` to `path` on Box `id`'s branch, as its agent would; the commit.
    fn push_to_branch_of(&self, id: BoxId, path: &str, text: &str) -> String {
        let branch = row(&self.w, id).branch.unwrap();
        git(&self.repo, &["switch", "-qc", &branch]);
        let pushed = self.commit(path, text);
        git(&self.repo, &["switch", "-q", "main"]);
        pushed
    }

    /// Takes Box `id`'s machine away and starts it again.
    fn lose_machine_of(&self, id: BoxId, name: &str, yaml: &str) -> StartView {
        let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
        self.w.cx.store().unwrap().apply(gone).unwrap();
        self.start(name, yaml).1
    }

    pub(super) fn warm(&self, id: BoxId) -> usize {
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
fn a_same_commit_fork_with_a_kept_tree_keeps_what_the_line_left_untracked_and_runs_it_again() {
    let s = Scratch::on(FakeDriver::new().cache_volumes());
    let (a, _) = s.start("a", LOOSE);
    assert!(s.tree(a).join("loose").exists());
    let (next, v) = s.start("next", LOOSE);
    assert_eq!(s.warm(next), 1);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(s.file(next, "loose"), "made\n");
    assert_eq!(events(&s.w, next, "setup_line_started").len(), 1);
}

#[test]
fn a_dirty_tree_s_rerun_at_the_same_commit_uses_one_attach_and_saves_nothing_on_every_box() {
    let yaml = "setup: ['true', 'echo made > loose', 'true']\n";
    let s = Scratch::on(FakeDriver::new().cache_volumes());
    let (a, _) = s.start("a", yaml);
    assert_eq!(setup_calls_on(&s.w, a), (3, 3), "a cold Box saves");
    let saved = s.w.driver.checkpoint_names();
    for name in ["next", "third"] {
        let (id, v) = s.start(name, yaml);
        assert_eq!(s.warm(id), 1);
        assert_eq!(s.file(id, "loose"), "made\n");
        assert_eq!(setup_calls_on(&s.w, id), (1, 1), "{name}");
        assert_eq!(s.w.driver.checkpoint_names(), saved, "{name}");
        assert_eq!(events(&s.w, id, "warm_rerun")[0]["changed"], 0);
        assert!(
            v.warnings_inline
                .iter()
                .any(|n| n.contains("every setup line ran")),
            "{v:?}"
        );
    }
}

#[test]
fn a_same_commit_fork_of_a_setup_that_left_only_ignored_files_still_skips_its_lines() {
    let s = Scratch::on(FakeDriver::new().cache_volumes());
    s.start("a", GUARD);
    let (next, _) = s.start("next", GUARD);
    assert_eq!(s.warm(next), 1);
    assert_eq!(s.file(next, "dist/out"), "one\n");
    assert_eq!(setup_calls_on(&s.w, next), (0, 0));
    assert!(events(&s.w, next, "warm_rerun").is_empty());
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

/// Three lines whose middle one runs the tracked script `check`.
const THREE: &str = "setup: ['true', 'sh ./check', 'echo last > \"$HOME/last\"']\n";

/// A scratch repo with a passing `check` and a Box that saved its layers.
fn after_three_lines() -> Scratch {
    let s = Scratch::new();
    s.commit("check", "exit 0\n");
    s.start("a", THREE);
    s
}

#[test]
fn a_rerun_in_one_script_reports_each_line() {
    let s = after_three_lines();
    s.commit("other", "changed\n");
    let (next, v) = s.start("next", THREE);
    assert_eq!(s.warm(next), 1);
    assert!(v.setup.ok, "{v:?}");
    let started = events(&s.w, next, "setup_line_started");
    let mut seen: Vec<_> = started.iter().map(|e| e["index"].as_u64()).collect();
    seen.sort_unstable();
    assert_eq!(seen, [Some(1), Some(2), Some(3)], "{started:?}");
    assert!(started.iter().all(|e| e["total"] == 3), "{started:?}");
    let finished = events(&s.w, next, "setup_line_finished");
    assert_eq!(finished.len(), 3, "{finished:?}");
    assert!(finished.iter().all(|e| e["ok"] == true), "{finished:?}");
}

#[test]
fn a_rerun_in_one_script_names_the_line_that_fails_and_tries_cold() {
    let s = after_three_lines();
    s.commit("check", "exit 3\n");
    let (failing, v) = s.start("failing", THREE);
    assert_eq!(s.warm(failing), 1);
    let failed = events(&s.w, failing, "warm_rerun_failed");
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["line"], "sh ./check");
    assert_eq!(row(&s.w, failing).lifecycle, BoxLifecycle::SetupFailed);
    assert_eq!(v.setup.line.as_deref(), Some("sh ./check"));
    assert!(
        !s.home(failing).join("last").exists(),
        "the cold try stopped there too"
    );
}

/// A Box at [`SEEN`] whose agent pushed, then lost its machine: its tree comes back with the push.
fn pushed_commits_come_back(driver: FakeDriver) {
    let s = Scratch::on(driver);
    let (a, _) = s.start("a", SEEN);
    let pushed = s.push_to_branch_of(a, "other", "pushed\n");
    let v = s.lose_machine_of(a, "a", SEEN);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(s.warm(a), 1, "it came back on a fork");
    let tree = s.tree(a);
    assert_eq!(git(&tree, &["rev-parse", "HEAD"]), pushed);
    assert_eq!(s.file(a, "other"), "pushed\n");
    assert_eq!(
        events(&s.w, a, "warm_rerun").len(),
        1,
        "off the base, lines rerun"
    );
}

#[test]
fn stay_kept_tree_keeps_pushed_commits() {
    pushed_commits_come_back(FakeDriver::new().cache_volumes());
}

#[test]
fn stay_no_kept_tree_keeps_pushed_commits() {
    pushed_commits_come_back(FakeDriver::new());
}

#[test]
fn a_same_commit_fork_leaves_no_ref_of_the_source_box() {
    for driver in [FakeDriver::new().cache_volumes(), FakeDriver::new()] {
        let s = Scratch::on(driver);
        s.start("a", LOOSE);
        let (next, _) = s.start("next", LOOSE);
        let heads = git(
            &s.tree(next),
            &["for-each-ref", "--format=%(refname)", "refs/heads"],
        );
        assert_eq!(
            heads,
            format!("refs/heads/{}", row(&s.w, next).branch.unwrap())
        );
        assert_eq!(s.file(next, "loose"), "made\n");
    }
}

#[test]
fn a_same_commit_fork_keeps_a_tracked_change_its_line_made_and_runs_the_line_again() {
    let yaml = "setup: ['echo x >> src']\n";
    let s = Scratch::on(FakeDriver::new().cache_volumes());
    s.start("a", yaml);
    let saved = s.w.driver.checkpoint_names();
    let (next, _) = s.start("next", yaml);
    assert!(
        s.file(next, "src").starts_with("one\nx\n"),
        "kept and run again"
    );
    assert_eq!(events(&s.w, next, "setup_line_started").len(), 1);
    assert_eq!(s.w.driver.checkpoint_names(), saved);
}

/// A first line that leaves a `git` on the Box's `PATH` whose `fetch` fails, so a later fork's
/// refresh falls back to a fresh clone, and a second line that makes an ignored output.
const FETCH_FAILS: &str = "setup: ['mkdir -p \"$HOME/bin\" && printf \"#!/bin/sh\\n[ \\\"\\$1\\\" = fetch ] && exit 1\\nexec /usr/bin/git \\\"\\$@\\\"\\n\" > \"$HOME/bin/git\" && chmod +x \"$HOME/bin/git\"', 'mkdir -p dist && cp src dist/out']\n";

#[test]
fn a_restart_after_a_failed_fetch_s_fresh_clone_still_runs_the_lines_it_lost() {
    let s = Scratch::new();
    let (a, _) = s.start("a", FETCH_FAILS);
    assert_eq!(s.file(a, "dist/out"), "one\n");
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    s.w.driver.on_exec_of("moor setup: $", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    s.w.forge
        .answer(repo(), Ok(repo_at(&s.head(), Some(FETCH_FAILS))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("next")
    };
    assert!(catch_unwind(AssertUnwindSafe(|| drop(run(&s.w.cx, &req)))).is_err());
    dying.store(false, Ordering::SeqCst);
    let store = s.w.cx.store().unwrap();
    let name = crate::testing::box_name("next");
    let next = store.open_box_by_name(&name).unwrap().unwrap().id;
    assert_eq!(s.warm(next), 1);
    fail_interrupted(&s.w.cx, store).unwrap();
    resume_setup(&s.w.cx, store, next).unwrap();
    assert_eq!(row(&s.w, next).lifecycle, BoxLifecycle::Ready);
    assert_eq!(s.file(next, "dist/out"), "one\n", "the output is back");
}

#[test]
fn a_moved_tag_keeps_a_same_commit_fork_warm() {
    let s = Scratch::new();
    git(&s.repo, &["tag", "nightly"]);
    s.start("a", GUARD);
    let base = s.head();
    s.commit("other", "changed\n");
    git(&s.repo, &["tag", "-f", "nightly"]);
    s.w.forge.answer(repo(), Ok(repo_at(&base, Some(GUARD))));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("next")
    };
    let v = run(&s.w.cx, &req).unwrap();
    let next = BoxId::new(v.boxed.id);
    assert_eq!(s.warm(next), 1);
    assert_eq!(
        events(&s.w, next, "setup_line_started").len(),
        0,
        "still warm"
    );
    assert_eq!(s.file(next, "dist/out"), "one\n");
}
