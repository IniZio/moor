//! The checkpoint above the setup lines, on real work trees: a later Box forks the agent install, the
//! profile and its plugins with the disk, and runs none of them.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_core::name::ForgeRepo;
use moor_core::seam::agent_adapter::AgentInstall;
use moor_core::seam::driver::ExecOutcome;
use moor_core::seam::engine::GitSource;
use moor_core::seam::forge::ForgeError;
use moor_core::{BoxId, BoxReq, Sha, StartReq};

use super::profile::set_profile;
use super::warm::events;
use super::warm_commits::setup_calls_on;
use super::warm_trees::{git, Scratch};
use super::{agentless_req, repo};
use crate::ops::common::provision::interrupted::fail_interrupted;
use crate::ops::common::provision::resume_setup;
use crate::ops::start::profile::SOURCE_LOOKUP;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::engine::FakeEngine;
use crate::testing::forge::{repo_at, FakeForge};

const LINES: &str = "setup: ['true', 'true']\n";

const MARKS: [&str; 3] = ["install-ran", "profile-ran", "plugins-ran"];

/// A scratch repo, and a profile repo beside it, for an Engine with an install and plugin installs.
pub(super) struct Profiled {
    pub(super) s: Scratch,
    profile: PathBuf,
    /// What each repo the profile installs from answers, as a commit or as nothing.
    sources: Vec<(ForgeRepo, Option<String>)>,
    /// The script the Engine runs for the plugin installs.
    plugins: String,
}

fn engine(plugins: &str) -> FakeEngine {
    FakeEngine::new()
        .install(AgentInstall {
            script: "echo x >> \"$HOME/install-ran\"; touch \"$HOME/has-agent\"".to_owned(),
            present: "[ -f \"$HOME/has-agent\" ]".to_owned(),
            dirs: Vec::new(),
            check: "true".to_owned(),
        })
        .profile_install(plugins)
}

impl Profiled {
    pub(super) fn new(setup_sh: &str, plugins: &str) -> Self {
        Self::on(FakeDriver::new(), setup_sh, plugins)
    }

    fn on(driver: FakeDriver, setup_sh: &str, plugins: &str) -> Self {
        Self::over(Scratch::on(driver), setup_sh, plugins)
    }

    /// A profile beside the scratch repo of `s`.
    pub(super) fn over(mut s: Scratch, setup_sh: &str, plugins: &str) -> Self {
        let profile = s.dir.path().join("ann/profile.git");
        std::fs::create_dir_all(&profile).unwrap();
        git(&profile, &["init", "-q", "-b", "main"]);
        git(
            &profile,
            &["config", "uploadpack.allowAnySHA1InWant", "true"],
        );
        s.w.cx = s.w.cx.with_engine(Arc::new(engine(plugins)));
        let p = Self {
            s,
            profile,
            sources: Vec::new(),
            plugins: plugins.to_owned(),
        }
        .commit_profile(setup_sh);
        set_profile(&p.s.w, None);
        p
    }

    /// Commits `setup_sh` as the profile's `setup.sh`, and makes it what a new Box resolves.
    pub(super) fn commit_profile(self, setup_sh: &str) -> Self {
        std::fs::write(self.profile.join("setup.sh"), setup_sh).unwrap();
        git(&self.profile, &["add", "."]);
        git(&self.profile, &["commit", "-qm", "p"]);
        let sha = git(&self.profile, &["rev-parse", "HEAD"]);
        self.answer_with(&sha)
    }

    /// Makes the profile resolve to `sha`, and each repo it installs from answer as `self.sources`.
    fn answer_with(mut self, sha: &str) -> Self {
        let mut forge = FakeForge::new().with_file(
            ForgeRepo::new("ann", "profile"),
            None,
            "moor-profile.yaml",
            Ok((Sha::parse(sha).unwrap(), Some("{}\n".to_owned()))),
        );
        for (repo, commit) in &self.sources {
            let url = format!("https://github.com/{repo}");
            let answer = commit.as_ref().map_or_else(
                || Err(ForgeError::Unavailable("it is down".to_owned())),
                |c| Ok((Sha::parse(c).unwrap(), None)),
            );
            forge = forge.with_repo(&url, repo.clone()).with_file(
                repo.clone(),
                None,
                SOURCE_LOOKUP,
                answer,
            );
        }
        let forge = Arc::new(forge);
        self.s.w.forge = forge.clone();
        self.s.w.cx = self.s.w.cx.with_forge(forge);
        self
    }

    /// The profile installs from `o/tools`, whose default branch is at `commit`, or which does not
    /// answer when none.
    fn tools_at(mut self, commit: Option<&str>) -> Self {
        let tools = GitSource {
            url: "https://github.com/o/tools".to_owned(),
            git_ref: None,
        };
        let engine = engine(&self.plugins).profile_sources(vec![tools]);
        self.s.w.cx = self.s.w.cx.with_engine(Arc::new(engine));
        self.sources = vec![(ForgeRepo::new("o", "tools"), commit.map(str::to_owned))];
        let sha = git(&self.profile, &["rev-parse", "HEAD"]);
        self.answer_with(&sha)
    }

    /// How many times each of the three steps has run on the disk of Box `id`.
    pub(super) fn runs_on_disk(&self, id: BoxId) -> [usize; 3] {
        MARKS.map(|m| lines_in(&self.s.home(id).join(m)))
    }

    /// The execs of the three steps on the machine of Box `id`.
    pub(super) fn step_execs(&self, id: BoxId) -> usize {
        let handle = self.s.handle(id);
        let execs = self.s.w.driver.execs();
        let on = execs.iter().filter(|(h, _, _)| h.as_str() == handle);
        on.filter(|(_, spec, _)| {
            let argv = spec.argv.join(" ");
            MARKS.iter().any(|m| argv.contains(m)) || argv.contains(".moor-profile/person")
        })
        .count()
    }

    pub(super) fn saved(&self) -> usize {
        self.s.w.driver.checkpoint_names().len()
    }

    pub(super) fn finished(&self, id: BoxId) -> bool {
        events(&self.s.w, id, "warm_started")[0]["finished"] == true
    }
}

fn lines_in(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().count())
}

pub(super) const SETUP_SH: &str = "echo x >> \"$HOME/profile-ran\"\n";

pub(super) const PLUGINS: &str = "echo x >> \"$HOME/plugins-ran\"\n";

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

const LINE: &str = "setup: ['echo line']\n";

#[test]
fn a_rerun_line_that_removes_the_agent_gets_it_back() {
    let yaml = "setup: ['rm -f \"$HOME/has-agent\"']\n";
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", yaml);
    p.s.commit("other", "y\n");
    let (b, v) = p.s.start("b", yaml);
    assert!(p.finished(b));
    assert!(p.s.home(b).join("has-agent").exists(), "{v:?}");
    assert_eq!(
        p.runs_on_disk(b),
        [2, 2, 2],
        "the steps ran again on the miss"
    );
}

#[test]
fn a_finished_fork_that_still_has_its_agent_runs_only_the_check() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", LINE);
    let (next, _) = p.s.start("next", LINE);
    assert!(p.finished(next));
    assert_eq!(p.step_execs(next), 0);
    let handle = p.s.handle(next);
    let execs = p.s.w.driver.execs();
    let checks = execs
        .iter()
        .filter(|(h, spec, _)| h.as_str() == handle && spec.argv.join(" ").contains("has-agent"));
    assert_eq!(checks.count(), 1);
}

/// A line that leaves an agent on its layer, as a cache volume can, so only the record can tell.
const KEEPS_AGENT: &str = "setup: ['touch $HOME/has-agent']\n";

#[test]
fn a_restart_of_a_retry_that_left_a_finished_fork_still_runs_the_steps() {
    let dies = ExecOutcome::Exited { code: 72 };
    let driver = FakeDriver::new().exec_ends_once_when("moor_tree_refresh\n", dies);
    let p = Profiled::on(driver, SETUP_SH, PLUGINS);
    p.s.start("a", KEEPS_AGENT);
    let at = repo_at(&p.s.head(), Some(KEEPS_AGENT));
    p.s.w.forge.answer(repo(), Ok(at));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("b")
    };
    assert!(run(&p.s.w.cx, &req).is_err(), "the fork's clone exec dies");
    let store = p.s.w.cx.store().unwrap();
    let name = crate::testing::box_name("b");
    let b = store.open_box_by_name(&name).unwrap().unwrap().id;
    let dying = Arc::new(AtomicBool::new(true));
    let crash = dying.clone();
    p.s.w.driver.on_exec_of("moor_tree_refresh\n", move || {
        assert!(!crash.load(Ordering::SeqCst), "the controller dies here");
    });
    let retry = BoxReq {
        box_name: crate::testing::box_name("b"),
    };
    let cx = &p.s.w.cx;
    assert!(
        catch_unwind(AssertUnwindSafe(|| drop(crate::ops::retry::run(
            cx, &retry
        ))))
        .is_err()
    );
    dying.store(false, Ordering::SeqCst);
    fail_interrupted(cx, store).unwrap();
    resume_setup(cx, store, b).unwrap();
    assert_eq!(p.runs_on_disk(b), [1, 1, 1]);
}

#[test]
fn a_changed_repo_env_runs_those_steps_and_no_setup_line() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", "setup: ['echo line']\nenv: {FOO: one}\n");
    let saved = p.saved();
    let (next, _) = p.s.start("next", "setup: ['echo line']\nenv: {FOO: two}\n");
    assert!(!p.finished(next));
    assert_eq!(setup_calls_on(&p.s.w, next), (0, 0));
    assert_eq!(p.runs_on_disk(next), [1, 1, 1], "over the line layer");
    assert_eq!(p.saved(), saved + 1);
}

/// Makes the Proxy's one rule put `TOOL_URL=value` in the env of later Boxes.
fn with_rule_env(mut p: Profiled, value: &str) -> Profiled {
    let yaml = format!(
        "proxy: {{rules: {{gitlab: {{hosts: [gitlab.com], credential: github, \
         inject: {{form: 'Bearer {{secret}}'}}, box_env: {{TOOL_URL: {value}}}}}}}}}"
    );
    let mut config = p.s.w.cx.config().clone();
    config.proxy = moor_core::config::parse_user("u.yaml", &yaml)
        .unwrap()
        .proxy;
    p.s.w.cx = p.s.w.cx.with_config(config);
    p
}

#[test]
fn a_changed_rule_env_runs_those_steps_and_no_setup_line() {
    let p = with_rule_env(Profiled::new(SETUP_SH, PLUGINS), "one");
    p.s.start("a", LINE);
    let (same, _) = p.s.start("same", LINE);
    assert!(p.finished(same), "the same env keeps the layer");
    let p = with_rule_env(p, "two");
    let (next, _) = p.s.start("next", LINE);
    assert!(!p.finished(next));
    assert_eq!(setup_calls_on(&p.s.w, next), (0, 0));
    assert_eq!(p.runs_on_disk(next), [1, 1, 1], "over the line layer");
}

/// A profile that installs from `o/tools`, at its first commit, with a Box that saved the layer.
fn after_a_box_with_tools() -> Profiled {
    let p = Profiled::new(SETUP_SH, PLUGINS).tools_at(Some(TOOLS_ONE));
    p.s.start("a", LINE);
    p
}

const TOOLS_ONE: &str = "1111111111111111111111111111111111111111";

const TOOLS_TWO: &str = "2222222222222222222222222222222222222222";

#[test]
fn a_source_at_the_same_commit_keeps_the_layer() {
    let p = after_a_box_with_tools();
    let (next, _) = p.s.start("next", LINE);
    assert!(p.finished(next));
    assert_eq!(p.step_execs(next), 0);
}

#[test]
fn a_new_commit_of_a_source_runs_those_steps_and_no_setup_line_then_saves_its_own_layer() {
    let p = after_a_box_with_tools();
    let saved = p.saved();
    let p = p.tools_at(Some(TOOLS_TWO));
    let (next, _) = p.s.start("next", LINE);
    assert!(!p.finished(next));
    assert_eq!(setup_calls_on(&p.s.w, next), (0, 0));
    assert_eq!(p.runs_on_disk(next), [1, 1, 1]);
    assert_eq!(p.saved(), saved + 1);
    let (third, _) = p.s.start("third", LINE);
    assert!(p.finished(third));
}

#[test]
fn a_source_that_does_not_resolve_is_a_miss_that_says_why_and_saves_nothing() {
    let p = after_a_box_with_tools();
    let saved = p.saved();
    let p = p.tools_at(None);
    p.s.w.cx.take_warnings();
    let (next, _) = p.s.start("next", LINE);
    assert!(!p.finished(next), "never the layer, silently");
    assert_eq!(p.runs_on_disk(next), [1, 1, 1]);
    assert_eq!(p.saved(), saved);
    let told = events(&p.s.w, next, "profile_resolved");
    assert!(
        told[0]["unresolved"][0]
            .as_str()
            .unwrap()
            .contains("o/tools"),
        "{told:?}"
    );
    let warned = p.s.w.cx.take_warnings();
    assert!(
        warned.iter().any(|x| x.code == "profile_source_unresolved"),
        "{warned:?}"
    );
}

#[test]
fn a_source_pinned_to_a_commit_needs_no_lookup_and_its_pin_is_in_the_key() {
    let pinned = |commit: &str| GitSource {
        url: "https://github.com/o/tools".to_owned(),
        git_ref: Some(commit.to_owned()),
    };
    let p = Profiled::new(SETUP_SH, PLUGINS);
    let with = |commit: &str| Arc::new(engine(PLUGINS).profile_sources(vec![pinned(commit)]));
    let mut p = p;
    p.s.w.cx = p.s.w.cx.with_engine(with(TOOLS_ONE));
    p.s.start("a", LINE);
    let (same, _) = p.s.start("same", LINE);
    assert!(p.finished(same));
    let asked = p.s.w.forge.files_asked();
    assert!(
        asked.iter().all(|(_, _, path)| *path != SOURCE_LOOKUP),
        "{asked:?}"
    );
    p.s.w.cx = p.s.w.cx.with_engine(with(TOOLS_TWO));
    let (moved, _) = p.s.start("moved", LINE);
    assert!(!p.finished(moved));
}
