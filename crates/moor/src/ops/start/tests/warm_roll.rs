//! A chain that moves with its repo: a rerun worth keeping is saved at its true base, and a disk that
//! made a line fail is retired once a cold machine shows the disk was the cause.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::layers::LayerKey;
use moor_core::{BoxId, BoxReq, StartReq};

use super::warm::{events, row};
use super::warm_commits::setup_calls_on;
use super::warm_profile::{Profiled, PLUGINS, SETUP_SH};
use super::warm_trees::{git, Scratch};
use super::{agentless_req, repo};
use crate::ops::common::provision::warm_disk_note;
use crate::ops::start::run;
use crate::store::checkpoints::CheckpointRow;
use crate::testing::forge::repo_at;

/// A line the clock takes 25 s over, past what earns a save.
const SLOW: &str = "setup: ['echo slow-marker']\n";

const QUICK: &str = "setup: ['echo quick']\n";

fn slow_scratch() -> Scratch {
    Scratch::new().slow_on("slow-marker", 25)
}

fn saved(s: &Scratch) -> usize {
    s.w.driver.checkpoint_names().len()
}

fn rerun_from(s: &Scratch, id: BoxId) -> serde_json::Value {
    events(&s.w, id, "warm_started")[0]["rerun_from"].clone()
}

#[test]
fn a_slow_rerun_is_saved_at_its_true_base_and_the_next_box_stands_near_its_commit() {
    let s = slow_scratch();
    let first = s.head();
    s.start("a", SLOW);
    let before = saved(&s);
    let second = s.commit("other", "y\n");
    let (moved, _) = s.start("b", SLOW);
    assert_eq!(rerun_from(&s, moved), first.as_str());
    assert!(saved(&s) > before, "the rerun is saved");
    let (here, _) = s.start("c", SLOW);
    assert!(rerun_from(&s, here).is_null(), "the disk is at its commit");
    assert_eq!(setup_calls_on(&s.w, here), (0, 0));
    assert_eq!(git(&s.tree(here), &["rev-parse", "HEAD"]), second);
    s.commit("other", "z\n");
    let (next, _) = s.start("d", SLOW);
    assert_eq!(
        rerun_from(&s, next),
        second.as_str(),
        "not the first commit"
    );
    assert_eq!(events(&s.w, next, "warm_rerun")[0]["changed"], 1);
}

#[test]
fn a_quick_rerun_of_lines_that_all_have_checkpoints_saves_nothing() {
    let s = Scratch::new();
    s.start("a", QUICK);
    let before = saved(&s);
    s.commit("other", "y\n");
    s.start("b", QUICK);
    assert_eq!(saved(&s), before);
}

#[test]
fn a_quick_rerun_with_a_new_line_saves_its_key_so_later_boxes_stop_paying_for_it() {
    let more = "setup: ['echo quick', 'echo new']\n";
    let s = Scratch::new();
    s.start("a", QUICK);
    let before = saved(&s);
    s.commit("other", "y\n");
    s.start("b", more);
    assert!(saved(&s) > before);
    let (c, _) = s.start("c", more);
    assert!(rerun_from(&s, c).is_null());
    assert_eq!(setup_calls_on(&s.w, c), (0, 0));
}

#[test]
fn a_new_profile_at_a_new_commit_is_saved_so_it_pays_once() {
    let p = Profiled::new(SETUP_SH, PLUGINS);
    p.s.start("a", QUICK);
    p.s.commit("other", "y\n");
    let p = p.commit_profile("echo y >> \"$HOME/profile-ran\"\n");
    let (b, _) = p.s.start("b", QUICK);
    assert!(p.step_execs(b) > 0, "it pays once");
    let (c, _) = p.s.start("c", QUICK);
    assert!(p.finished(c));
    assert_eq!(p.step_execs(c), 0);
    assert!(rerun_from(&p.s, c).is_null());
    assert_eq!(setup_calls_on(&p.s.w, c), (0, 0));
}

#[test]
fn a_slow_rerun_on_a_fork_of_the_steps_saves_them_at_its_commit() {
    let p = Profiled::over(slow_scratch(), SETUP_SH, PLUGINS);
    p.s.start("a", SLOW);
    p.s.commit("other", "y\n");
    let (b, _) = p.s.start("b", SLOW);
    assert!(p.finished(b));
    let (c, _) = p.s.start("c", SLOW);
    assert!(p.finished(c));
    assert!(rerun_from(&p.s, c).is_null(), "the disk is at its commit");
    assert_eq!(setup_calls_on(&p.s.w, c), (0, 0));
}

/// A slow line that also removes the agent, as a lost cache can.
const SLOW_AND_LOSES_AGENT: &str = "setup: ['echo slow-marker; rm -f $HOME/has-agent']\n";

#[test]
fn a_fork_of_the_steps_that_lost_its_agent_never_saves_them_as_its_line() {
    let p = Profiled::over(slow_scratch(), SETUP_SH, PLUGINS);
    p.s.start("a", SLOW_AND_LOSES_AGENT);
    p.s.commit("other", "y\n");
    let (b, _) = p.s.start("b", SLOW_AND_LOSES_AGENT);
    assert_eq!(p.runs_on_disk(b), [2, 2, 2], "it got its agent back");
    let p = p.commit_profile("echo y >> \"$HOME/profile-ran\"\n");
    let (c, _) = p.s.start("c", SLOW_AND_LOSES_AGENT);
    assert_eq!(
        p.runs_on_disk(c),
        [1, 1, 1],
        "a line layer held none of them"
    );
}

#[test]
fn a_slow_rerun_that_leaves_the_tree_dirty_is_not_saved_and_says_so() {
    let dirty = "setup: ['echo slow-marker > loose']\n";
    let s = slow_scratch();
    s.start("a", dirty);
    let before = saved(&s);
    s.commit("other", "y\n");
    s.w.cx.take_warnings();
    s.start("b", dirty);
    assert_eq!(saved(&s), before);
    let warned = s.w.cx.take_warnings();
    assert!(
        warned.iter().any(|x| x.code == "checkpoint_skipped"),
        "{warned:?}"
    );
}

#[test]
fn a_slow_rerun_that_moves_head_is_not_saved() {
    let moves = "setup: ['echo slow-marker; [ -f other ] && git -c user.name=t -c user.email=t@t \
                 -c commit.gpgsign=false commit -q --allow-empty -m moved; true']\n";
    let s = slow_scratch();
    s.start("a", moves);
    let before = saved(&s);
    s.commit("other", "y\n");
    let (b, _) = s.start("b", moves);
    assert_ne!(git(&s.tree(b), &["rev-parse", "HEAD"]), s.head());
    assert_eq!(saved(&s), before);
}

#[test]
fn a_rerun_on_a_branch_already_pushed_is_not_saved_and_needs_no_check() {
    let s = slow_scratch();
    s.start("a", SLOW);
    let before = saved(&s);
    git(&s.repo, &["switch", "-qc", "moor/2-b"]);
    let pushed = s.commit("pushed", "p\n");
    git(&s.repo, &["switch", "-q", "main"]);
    s.commit("other", "y\n");
    s.w.cx.take_warnings();
    let (b, _) = s.start("b", SLOW);
    assert_eq!(git(&s.tree(b), &["rev-parse", "HEAD"]), pushed);
    assert!(rerun_from(&s, b).is_string());
    assert_eq!(saved(&s), before);
    let warned = s.w.cx.take_warnings();
    assert!(
        warned.iter().all(|x| x.code != "checkpoint_skipped"),
        "{warned:?}"
    );
}

/// A line that fails only over a disk from when `src` was `one`.
const POISONED: &str =
    "setup: ['[ ! -f dist/old ] || exit 3; [ \"$(cat src)\" != one ] || { mkdir \
                        -p dist; touch dist/old; }']\n";

#[test]
fn a_disk_that_failed_its_rerun_is_replaced_by_the_cold_machine_and_no_later_box_pays() {
    let s = Scratch::new();
    s.start("a", POISONED);
    s.commit("src", "two\n");
    let (cold, v) = s.start("b", POISONED);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(events(&s.w, cold, "warm_rerun_failed").len(), 1);
    let (next, _) = s.start("c", POISONED);
    assert_eq!(events(&s.w, next, "warm_rerun_failed").len(), 0, "retired");
    assert_eq!(
        events(&s.w, next, "warm_started").len(),
        1,
        "it forked the cold Box's"
    );
    assert!(rerun_from(&s, next).is_null());
    s.commit("other", "z\n");
    let (later, _) = s.start("d", POISONED);
    assert_eq!(events(&s.w, later, "warm_rerun_failed").len(), 0);
    assert_eq!(events(&s.w, later, "warm_started").len(), 1);
}

#[test]
fn a_line_that_also_fails_cold_keeps_the_chain_for_the_commit_that_fixes_it() {
    let yaml = "setup: ['sh ./check']\n";
    let s = Scratch::new();
    s.commit("check", "exit 0\n");
    s.start("a", yaml);
    s.commit("check", "exit 3\n");
    let at = repo_at(&s.head(), Some(yaml));
    s.w.forge.answer(repo(), Ok(at));
    let req = StartReq {
        repo: Some(repo()),
        ..agentless_req("broken")
    };
    let v = run(&s.w.cx, &req).unwrap();
    let broken = BoxId::new(v.boxed.id);
    assert_eq!(row(&s.w, broken).lifecycle, BoxLifecycle::SetupFailed);
    s.commit("check", "exit 0 # fixed\n");
    let (fixed, _) = s.start("fixed", yaml);
    let warm = events(&s.w, fixed, "warm_started");
    assert_eq!(warm[0]["layer"], 1, "the chain's top lives: {warm:?}");
    assert_eq!(events(&s.w, fixed, "warm_rerun_failed").len(), 0);
}

#[test]
fn a_rolled_checkpoint_records_its_commit_its_depth_and_its_chain_s_cold_time() {
    let s = slow_scratch();
    let (a, _) = s.start("a", SLOW);
    let first = kept_last_saved_by(&s, a);
    assert_eq!(first.depth, 0);
    let y = s.commit("other", "y\n");
    let (b, _) = s.start("b", SLOW);
    let rolled = kept_last_saved_by(&s, b);
    assert_eq!(rolled.depth, 1);
    assert_eq!(rolled.chain_at, first.chain_at);
    let place = rolled.place.unwrap();
    assert_eq!(place.base, y);
    assert_eq!(
        place.setup_ms,
        first.place.unwrap().setup_ms,
        "still what it took cold"
    );
}

/// The row of the last checkpoint Box `id` saved.
fn kept_last_saved_by(s: &Scratch, id: BoxId) -> CheckpointRow {
    let taken = events(&s.w, id, "checkpoint_taken");
    let key = taken.last().unwrap()["key"].as_str().unwrap().to_owned();
    let key = LayerKey::restore(key).unwrap();
    let provider = row(&s.w, id).boxed.unwrap().provider;
    let store = s.w.cx.store().unwrap();
    store.checkpoint(&provider, &key).unwrap().unwrap()
}

#[test]
fn a_disk_that_failed_its_rerun_takes_the_rest_of_its_chain_with_it() {
    let line =
        "[ ! -f dist/old ] || exit 3; echo slow-marker; [ \"$(cat src)\" != one ] || { mkdir \
                -p dist; touch dist/old; }";
    let (old, new) = (
        format!("setup: ['{line}', 'echo old']\n"),
        format!("setup: ['{line}', 'echo new']\n"),
    );
    let s = slow_scratch();
    s.start("a", &old);
    s.commit("src", "two\n");
    let (b, v) = s.start("b", &new);
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(events(&s.w, b, "warm_rerun_failed").len(), 1);
    let (c, _) = s.start("c", &old);
    assert_eq!(
        events(&s.w, c, "warm_rerun_failed").len(),
        0,
        "the old top is gone"
    );
    assert!(rerun_from(&s, c).is_null(), "it forked this commit's layer");
}

/// A line whose guard skips it once its output is there, whatever `src` holds now.
const GUARD: &str = "setup: ['[ -f dist/out ] || { mkdir -p dist && cp src dist/out; }']\n";

/// [`GUARD`] with a line after it, which gets a key no checkpoint holds.
const GUARD_AND_MORE: &str =
    "setup: ['[ -f dist/out ] || { mkdir -p dist && cp src dist/out; }', 'true']\n";

/// A disk set up from scratch at its first commit with [`GUARD`], then rolled forward to a commit
/// that changes `src`; returns the scratch repo and that first commit.
fn rolled_over_a_guard() -> (Scratch, String) {
    let scratch = Scratch::new();
    let first = scratch.head();
    scratch.start("a", GUARD);
    scratch.commit("src", "two\n");
    let (rolled, _) = scratch.start("b", GUARD_AND_MORE);
    assert!(
        !rerun_from(&scratch, rolled).is_null(),
        "it reran and rolled forward"
    );
    (scratch, first)
}

#[test]
fn a_rolled_forward_disk_keeps_its_stale_output_and_the_next_box_hears_nothing() {
    let (scratch, first) = rolled_over_a_guard();
    let (next, _) = scratch.start("c", GUARD_AND_MORE);
    assert!(
        rerun_from(&scratch, next).is_null(),
        "it forks its own commit"
    );
    assert_eq!(setup_calls_on(&scratch.w, next), (0, 0));
    assert_eq!(
        scratch.file(next, "dist/out"),
        "one\n",
        "the output is still the first's"
    );
    let told = events(&scratch.w, next, "warm_rerun");
    assert_eq!(told.len(), 1, "it is told of the disk");
    assert_eq!(told[0]["commit"], first.as_str());
    assert_eq!(told[0]["changed"], 1);
    assert_eq!(told[0]["lines_kept"], true);
    let note = warm_disk_note(scratch.w.cx.store().unwrap(), next).expect("a note");
    let short = first.get(..12).unwrap();
    assert!(note.contains(short) && note.contains("dist/"), "{note}");
    assert!(note.contains("did not run again"), "{note}");
}

#[test]
fn a_box_at_a_later_commit_hears_of_the_oldest_commit_not_the_roll_s() {
    let (scratch, first) = rolled_over_a_guard();
    scratch.commit("other", "z\n");
    let (later, _) = scratch.start("d", GUARD_AND_MORE);
    let told = events(&scratch.w, later, "warm_rerun");
    assert_eq!(told[0]["commit"], first.as_str());
    assert_eq!(told[0]["changed"], 2);
    assert!(told[0].get("lines_kept").is_none(), "its lines ran again");
}

#[test]
fn a_disk_set_up_from_scratch_at_its_commit_needs_no_note() {
    let s = Scratch::new();
    s.start("a", GUARD);
    let (b, _) = s.start("b", GUARD);
    assert_eq!(s.warm(b), 1);
    assert!(events(&s.w, b, "warm_rerun").is_empty());
}

/// Makes every checkpoint of `scratch` look as one that rolled forward before moor kept the oldest
/// commit its ignored files may come from.
fn as_rolled_before_it_was_kept(scratch: &Scratch) {
    use crate::store::db::DbExt as _;
    use moor_schema::tables::SetupCheckpoints as Checkpoints;
    use sea_query::{Expr, ExprTrait as _, Query};

    let store = scratch.w.cx.store().unwrap();
    let forget = Query::update()
        .table(Checkpoints::Table)
        .value(Checkpoints::StaleSince, Expr::val(None::<String>))
        .take();
    store.writer().run(forget).unwrap();
    let rolled = Query::update()
        .table(Checkpoints::Table)
        .value(Checkpoints::Depth, 1)
        .and_where(Expr::col(Checkpoints::Evicting).eq(0))
        .take();
    store.writer().run(rolled).unwrap();
    scratch.w.cx.take_warnings();
}

#[test]
fn a_disk_that_rolled_forward_before_moor_kept_its_oldest_commit_is_set_up_again_once() {
    let (scratch, _) = rolled_over_a_guard();
    as_rolled_before_it_was_kept(&scratch);
    let (cold, _) = scratch.start("c", GUARD_AND_MORE);
    assert_eq!(scratch.warm(cold), 0, "no Box forks a disk of unknown age");
    let warned = scratch.w.cx.take_warnings();
    let said = warned
        .iter()
        .any(|n| n.message.contains("rolled forward before"));
    assert!(said, "{warned:?}");
    assert_eq!(scratch.file(cold, "dist/out"), "two\n");
    let (next, _) = scratch.start("d", GUARD_AND_MORE);
    assert_eq!(scratch.warm(next), 1);
    assert!(
        events(&scratch.w, next, "warm_rerun").is_empty(),
        "set up from scratch"
    );
}

#[test]
fn a_retry_never_forks_a_disk_of_unknown_age() {
    let (scratch, _) = rolled_over_a_guard();
    let lines = "setup: ['[ -f dist/out ] || { mkdir -p dist && cp src dist/out; }', \
                 '[ -f \"$HOME/second\" ] || { touch \"$HOME/second\"; exit 1; }']\n";
    let (failed, _) = scratch.start("c", lines);
    assert_eq!(row(&scratch.w, failed).lifecycle, BoxLifecycle::SetupFailed);
    as_rolled_before_it_was_kept(&scratch);
    let forks = scratch.warm(failed);
    let retry = BoxReq {
        box_name: crate::testing::box_name("c"),
    };
    let v = crate::ops::retry::run(&scratch.w.cx, &retry).unwrap();
    assert!(v.setup.ok, "{v:?}");
    assert_eq!(scratch.warm(failed), forks, "the retry forks nothing");
}
