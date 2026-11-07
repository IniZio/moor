//! The checkpoint above the setup lines, on real work trees: a later Box forks the agent install, the
//! profile and its plugins with the disk, and runs none of them.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::name::ForgeRepo;
use moor_core::seam::agent_adapter::AgentInstall;
use moor_core::{BoxId, Sha, StartReq};

use super::profile::set_profile;
use super::warm::events;
use super::warm_commits::setup_calls_on;
use super::warm_trees::{git, Scratch};
use super::{agentless_req, repo};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::{repo_at, FakeForge};

const LINES: &str = "setup: ['true', 'true']\n";

const MARKS: [&str; 3] = ["install-ran", "profile-ran", "plugins-ran"];

/// A scratch repo, and a profile repo beside it, for an Engine with an install and plugin installs.
struct Profiled {
    s: Scratch,
    profile: PathBuf,
}

fn engine(plugins: &str) -> FakeEngine {
    FakeEngine::new()
        .install(AgentInstall {
            script: "echo x >> \"$HOME/install-ran\"".to_owned(),
            dirs: Vec::new(),
            check: "true".to_owned(),
        })
        .profile_install(plugins)
}

impl Profiled {
    fn new(setup_sh: &str, plugins: &str) -> Self {
        let mut s = Scratch::on(FakeDriver::new());
        let profile = s.dir.path().join("ann/profile.git");
        std::fs::create_dir_all(&profile).unwrap();
        git(&profile, &["init", "-q", "-b", "main"]);
        git(
            &profile,
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        s.w.cx = s.w.cx.with_engine(Arc::new(engine(plugins)));
        let p = Self { s, profile }.commit_profile(setup_sh);
        set_profile(&p.s.w, None);
        p
    }

    /// Commits `setup_sh` as the profile's `setup.sh`, and makes it what a new Box resolves.
    fn commit_profile(mut self, setup_sh: &str) -> Self {
        std::fs::write(self.profile.join("setup.sh"), setup_sh).unwrap();
        git(&self.profile, &["add", "."]);
        git(&self.profile, &["commit", "-qm", "p"]);
        let sha = git(&self.profile, &["rev-parse", "HEAD"]);
        let forge = Arc::new(FakeForge::new().with_file(
            ForgeRepo::new("ann", "profile"),
            None,
            "moor-profile.yaml",
            Ok((Sha::parse(&sha).unwrap(), Some("{}\n".to_owned()))),
        ));
        self.s.w.forge = forge.clone();
        self.s.w.cx = self.s.w.cx.with_forge(forge);
        self
    }

    /// How many times each of the three steps has run on the disk of Box `id`.
    fn runs_on_disk(&self, id: BoxId) -> [usize; 3] {
        MARKS.map(|m| lines_in(&self.s.home(id).join(m)))
    }

    /// The execs of the three steps on the machine of Box `id`.
    fn step_execs(&self, id: BoxId) -> usize {
        let handle = self.s.handle(id);
        let execs = self.s.w.driver.execs();
        let on = execs.iter().filter(|(h, _, _)| h.as_str() == handle);
        on.filter(|(_, spec, _)| {
            let argv = spec.argv.join(" ");
            MARKS.iter().any(|m| argv.contains(m)) || argv.contains(".moor-profile/person")
        })
        .count()
    }

    fn saved(&self) -> usize {
        self.s.w.driver.checkpoint_names().len()
    }

    fn finished(&self, id: BoxId) -> bool {
        events(&self.s.w, id, "warm_started")[0]["finished"] == true
    }
}

fn lines_in(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().count())
}

const SETUP_SH: &str = "echo x >> \"$HOME/profile-ran\"\n";

const PLUGINS: &str = "echo x >> \"$HOME/plugins-ran\"\n";

#[test]
fn a_second_box_runs_no_install_profile_or_plugin_exec_and_holds_what_they_left() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    let (a, v) = p.s.start("a", LINES);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(p.runs_on_disk(a), [1, 1, 1]);
    assert_eq!(setup_calls_on(&p.s.w, a), (2, 2));
    let saved = p.saved();
    let (next, _) = p.s.start("next", LINES);
    assert!(p.finished(next));
    assert_eq!(p.step_execs(next), 0);
    assert_eq!(p.runs_on_disk(next), [1, 1, 1], "held, not run again");
    assert_eq!(setup_calls_on(&p.s.w, next), (0, 0));
    assert_eq!(p.saved(), saved, "a fork of the top layer saves nothing");
    let logins = p.s.w.driver.refreshed_logins();
    assert!(
        logins.iter().any(|(h, _)| h.as_str() == p.s.handle(next)),
        "{logins:?}"
    );
}

#[test]
fn a_profile_change_runs_those_steps_and_no_setup_line_then_saves_its_own_layer() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", LINES);
    let saved = p.saved();
    let p = p.commit_profile("echo y >> \"$HOME/profile-ran\"\n");
    let (next, _) = p.s.start("next", LINES);
    assert!(!p.finished(next));
    assert_eq!(
        setup_calls_on(&p.s.w, next),
        (0, 0),
        "the lines stay on the disk"
    );
    assert_eq!(
        p.runs_on_disk(next),
        [1, 1, 1],
        "the fork stood under those steps"
    );
    assert_eq!(p.saved(), saved + 1);
    let (third, _) = p.s.start("third", LINES);
    assert!(p.finished(third));
    assert_eq!(p.step_execs(third), 0);
}

#[test]
fn a_failed_setup_sh_saves_no_layer_and_the_next_box_tries_again_and_says_so() {
    let p = Profiled::new("exit 3\n", PLUGINS);
    let (a, _) = p.s.start("a", LINES);
    assert_eq!(events(&p.s.w, a, "profile_failed").len(), 1);
    let names = p.saved();
    let (next, _) = p.s.start("next", LINES);
    assert!(!p.finished(next));
    assert_eq!(events(&p.s.w, next, "profile_failed").len(), 1);
    assert_eq!(p.saved(), names);
}

#[test]
fn a_failed_plugin_install_saves_no_layer_and_names_the_plugin() {
    let p = Profiled::new(SETUP_SH, "echo 'moor: profile: plugin lint exit 7' >&2\n");
    let (a, _) = p.s.start("a", LINES);
    let failed = events(&p.s.w, a, "profile_failed");
    assert_eq!(failed[0]["detail"], "plugin lint exit 7", "{failed:?}");
    let saved = p.saved();
    let (next, _) = p.s.start("next", LINES);
    assert!(!p.finished(next));
    assert_eq!(p.saved(), saved);
}

#[test]
fn a_rerun_at_a_new_commit_stands_on_the_layer_and_runs_only_its_lines() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", LINES);
    let saved = p.saved();
    p.s.commit("other", "changed\n");
    let (next, v) = p.s.start("next", LINES);
    assert!(v.setup.ok, "{v:?}");
    assert!(p.finished(next));
    assert!(events(&p.s.w, next, "warm_started")[0]["rerun_from"].is_string());
    assert_eq!(setup_calls_on(&p.s.w, next), (1, 1), "the lines ran");
    assert_eq!(p.step_execs(next), 0);
    assert_eq!(p.runs_on_disk(next), [1, 1, 1]);
    assert_eq!(p.saved(), saved);
}

#[test]
fn a_rerun_with_a_changed_profile_runs_those_steps_and_saves_nothing() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", LINES);
    let saved = p.saved();
    p.s.commit("other", "changed\n");
    let p = p.commit_profile("echo y >> \"$HOME/profile-ran\"\n");
    let (next, _) = p.s.start("next", LINES);
    assert!(!p.finished(next));
    assert_eq!(setup_calls_on(&p.s.w, next), (1, 1));
    assert_eq!(p.runs_on_disk(next), [1, 1, 1]);
    assert_eq!(p.saved(), saved);
}

#[test]
fn a_line_that_fails_on_a_finished_rerun_goes_cold_and_runs_those_steps_there() {
    let yaml = "setup: ['if [ -f dist/once ]; then exit 3; fi; mkdir -p dist; touch dist/once']\n";
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", yaml);
    p.s.commit("other", "changed\n");
    let (next, v) = p.s.start("next", yaml);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(events(&p.s.w, next, "warm_rerun_failed").len(), 1);
    assert_eq!(p.runs_on_disk(next), [1, 1, 1], "a cold machine ran them");
    assert!(p.step_execs(next) > 0);
}

#[test]
fn a_restart_during_a_rerun_on_the_layer_still_runs_none_of_those_steps() {
    let yaml = "setup: ['echo marker']\n";
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", yaml);
    p.s.commit("other", "changed\n");
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    p.s.w.driver.on_exec_of("echo marker", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    let at = repo_at(&p.s.head(), Some(yaml));
    p.s.w.forge.answer(repo(), Ok(at));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("next")
    };
    assert!(catch_unwind(AssertUnwindSafe(|| drop(run(&p.s.w.cx, &req)))).is_err());
    dying.store(false, Ordering::SeqCst);
    let store = p.s.w.cx.store().unwrap();
    let name = crate::testing::box_name("next");
    let next = store.open_box_by_name(&name).unwrap().unwrap().id;
    fail_interrupted(&p.s.w.cx, store).unwrap();
    resume_setup(&p.s.w.cx, store, next).unwrap();
    assert_eq!(p.step_execs(next), 0);
    assert_eq!(p.runs_on_disk(next), [1, 1, 1]);
}
