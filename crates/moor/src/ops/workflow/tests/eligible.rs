//! Eligibility: the App reads each pull request back from GitHub and acts on what it says now.
//! A draft or a bot's pull request records nothing, a fork fires only for an author who can
//! write, a late older head never cancels the newer run, and a failed read shows as a refused run.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]

use moor_core::request::GitHubHookValue;
use moor_core::workflow::{
    CancelCause, CommitSha, PullRef, PullTarget, RunCause, RunEnd, RunState,
};

use super::hook::{body, cancelled, delivery, hooked, Hooked, NEW, OLD, SECRET};
use super::repo;
use crate::adapters::github::app::fake::pull_json;
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::{pull, run_start};
use crate::testing::cx::REQUESTER_USER;

impl Hooked {
    /// Delivers a signed `action` on pull request 65 whose payload names `head`, leaving the fake
    /// GitHub as the test set it.
    fn raw(&self, action: &str, head: &str) {
        let req = delivery(
            "pull_request",
            body(action, ("o/n", 65), head),
            Some(SECRET),
        );
        assert_eq!(self.rig.deliver(&req).unwrap(), GitHubHookValue::Accepted);
    }

    /// Sets pull request 65 on GitHub: open or not, a draft or not, at `head` in `head_repo`, by
    /// `author` of type `kind`.
    fn github_has(
        &self,
        (open, draft): (bool, bool),
        (head, head_repo): (&str, &str),
        author: (&str, &str),
    ) {
        self.github
            .set_pull(65, pull_json((open, draft), (head, head_repo), author));
    }
}

fn running(state: &RunState) -> bool {
    matches!(state, RunState::Running { .. })
}

#[test]
fn a_draft_records_nothing_and_ready_for_review_at_the_same_head_fires() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github_has((true, true), (OLD, "o/n"), ("alice", "User"));
    r.raw("opened", OLD);
    assert!(r.runs(id).is_empty(), "a draft writes no row");
    r.github_has((true, false), (OLD, "o/n"), ("alice", "User"));
    r.raw("ready_for_review", OLD);
    assert!(running(&r.one_at(id, 65, OLD).state));
}

#[test]
fn a_payload_that_says_ready_does_not_fire_a_pull_request_github_has_as_a_draft() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github_has((true, true), (OLD, "o/n"), ("alice", "User"));
    r.raw("ready_for_review", OLD);
    assert!(r.runs(id).is_empty());
}

#[test]
fn a_bot_author_records_nothing_by_type_or_by_login() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    for author in [("dependabot[bot]", "Bot"), ("renovate[bot]", "User")] {
        r.github_has((true, false), (OLD, "o/n"), author);
        r.raw("opened", OLD);
        assert!(r.runs(id).is_empty(), "{author:?}");
    }
}

#[test]
fn a_fork_whose_author_cannot_write_records_skipped_fork_with_the_author() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github.set_permission("mallory", "read");
    r.github_has((true, false), (OLD, "mallory/n"), ("mallory", "User"));
    r.raw("opened", OLD);
    let run = r.one_at(id, 65, OLD);
    assert_eq!(
        run.state,
        RunState::Done(RunEnd::SkippedFork {
            author: "mallory".to_owned()
        })
    );
    assert_eq!(run.cause.trigger().as_str(), "pull_request");
    assert_eq!(r.open_boxes(), 0);
    let asked = r.github.api.seen();
    assert!(
        asked
            .iter()
            .any(|s| s.path == "/repos/o/n/collaborators/mallory/permission"),
        "{asked:?}"
    );
}

#[test]
fn a_fork_whose_author_can_write_fires_and_a_same_repo_head_never_reads_the_permission() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    for (n, login, permission) in [(65, "alice", "write"), (66, "carol", "admin")] {
        r.github.set_permission(login, permission);
        let fork = format!("{login}/n");
        r.github
            .set_pull(n, pull_json((true, false), (OLD, &fork), (login, "User")));
        let req = delivery(
            "pull_request",
            body("opened", ("o/n", n), OLD),
            Some(SECRET),
        );
        r.rig.deliver(&req).unwrap();
        assert!(running(&r.one_at(id, n, OLD).state), "{login}");
    }
    r.github
        .set_pull(67, pull_json((true, false), (OLD, "O/N"), ("dave", "User")));
    let req = delivery(
        "pull_request",
        body("opened", ("o/n", 67), OLD),
        Some(SECRET),
    );
    r.rig.deliver(&req).unwrap();
    assert!(running(&r.one_at(id, 67, OLD).state));
    let asked = r.github.api.seen();
    assert!(
        !asked.iter().any(|s| s.path.contains("/dave/")),
        "{asked:?}"
    );
}

#[test]
fn run_again_on_a_skipped_fork_runs_its_current_head_as_the_owner_and_a_later_push_skips_again() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github_has((true, false), (OLD, "mallory/n"), ("mallory", "User"));
    r.raw("opened", OLD);
    assert!(matches!(
        r.one_at(id, 65, OLD).state,
        RunState::Done(RunEnd::SkippedFork { .. })
    ));
    r.pause(1_000);
    let target = PullTarget::parse("o/n#65").unwrap();
    let read = |t: &PullTarget| pull::read_head(&r.cx, t);
    let start = |job| {
        run_start(&r.cx, &job);
        Ok(())
    };
    let store = r.cx.store().unwrap();
    let args = ("evidence", None, Some(&target));
    let line = run_with(&r.cx, store, REQUESTER_USER, args, (&read, &start)).unwrap();
    assert_eq!(line.status, "started", "{line:?}");
    let manual = r.latest(id);
    let pull = PullRef {
        repo: repo(),
        number: 65,
        head_sha: CommitSha::parse(OLD).unwrap(),
    };
    assert_eq!(manual.cause, RunCause::Manual { pull: Some(pull) });
    assert!(running(&manual.state));
    r.pause(1_000);
    r.github_has((true, false), (NEW, "mallory/n"), ("mallory", "User"));
    r.raw("synchronize", NEW);
    assert!(matches!(
        r.one_at(id, 65, NEW).state,
        RunState::Done(RunEnd::SkippedFork { .. })
    ));
}

#[test]
fn a_late_older_head_never_recorded_leaves_the_newer_run_be() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github_has((true, false), (NEW, "o/n"), ("alice", "User"));
    r.raw("synchronize", NEW);
    r.pause(1_000);
    r.raw("opened", OLD);
    assert!(running(&r.one_at(id, 65, NEW).state));
    assert_eq!(r.runs(id).len(), 1, "{:?}", r.runs(id));
}

#[test]
fn a_late_push_to_a_pull_request_github_has_closed_cancels_and_fires_nothing() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    r.github_has((false, false), (NEW, "o/n"), ("alice", "User"));
    r.raw("synchronize", NEW);
    assert_eq!(
        cancelled(&r.one_at(id, 65, OLD)),
        Some(&CancelCause::Closed)
    );
    assert!(r.runs_at(id, 65, NEW).is_empty());
}

#[test]
fn a_failed_read_is_a_refused_run_with_the_reason_and_is_not_retried() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.github.set_down(true);
    r.raw("opened", OLD);
    let run = r.one_at(id, 65, OLD);
    let RunState::Done(RunEnd::Refused { detail }) = &run.state else {
        panic!("{:?}", run.state);
    };
    assert!(
        detail.contains("could not read o/n#65 from GitHub") && detail.contains("HTTP 503"),
        "{detail}"
    );
    assert_eq!(r.open_boxes(), 0);
    let asked = r.github.api.seen().len();
    r.raw("opened", OLD);
    assert_eq!(r.runs(id).len(), 1, "the same head again records nothing");
    assert_eq!(
        r.github.api.seen().len(),
        asked + 1,
        "one read per delivery, no retry"
    );
}

#[test]
fn a_close_whose_read_fails_cancels_nothing() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    r.github.set_down(true);
    r.raw("closed", OLD);
    assert!(running(&r.one_at(id, 65, OLD).state));
}
