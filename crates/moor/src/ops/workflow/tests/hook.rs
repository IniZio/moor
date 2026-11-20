//! The App's webhook: a signed `pull_request` delivery fires every enabled workflow of its repo
//! that has the trigger on, a newer head cancels the older run, a close cancels the open run even
//! while its Box is being started, and the same head twice claims once. Here the fake GitHub the
//! App reads back agrees with each payload; [`super::eligible`] tests where they differ.
#![allow(
    clippy::panic,
    clippy::cognitive_complexity,
    reason = "a test names what it found when it fails, and walks one scenario step by step"
)]

use std::cell::RefCell;
use std::fmt::Write as _;
use std::ops::Deref;

use hmac::{Hmac, Mac as _};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::request::{GitHubHookReq, GitHubHookValue};
use moor_core::user::Caller;
use moor_core::workflow::{
    CancelCause, RunEnd, RunOutcome, RunState, TimeoutMinutes, WorkflowDef, WorkflowId,
    WorkflowName, WorkflowRun, WorkflowSchedule,
};
use moor_core::BoxName;
use serde_json::{json, Value};
use sha2::Sha256;

use super::previous::lifecycle;
use super::{repo, rig, Rig};
use crate::adapters::github::app::fake::{pull_json, PullsApi};
use crate::adapters::github::hook::HookSecret;
use crate::error::CommandError;
use crate::ops::github_hook::{check, handle_with};
use crate::ops::workflow::{run_start, StartJob};
use crate::testing::cx::REQUESTER_USER;

/// The secret the test controller holds.
pub(super) const SECRET: &[u8] = b"test-webhook-secret";
/// One head of a pull request.
pub(super) const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// A head pushed after [`OLD`].
pub(super) const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// `sha256=` and the hex HMAC of `body` under `key`, as GitHub signs.
fn sign(key: &[u8], body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(body.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .fold(String::from("sha256="), |mut s, b| {
            write!(s, "{b:02x}").unwrap();
            s
        })
}

/// A `pull_request` body for `action` on pull request `number` of `repo` at `head`.
pub(super) fn body(action: &str, (repo, number): (&str, u64), head: &str) -> String {
    json!({
        "action": action,
        "number": number,
        "pull_request": {"number": number, "draft": false, "head": {"sha": head}},
        "repository": {"full_name": repo},
        "sender": {"login": "someone"},
    })
    .to_string()
}

/// A delivery of `event` with `body`, signed with `key` when there is one.
pub(super) fn delivery(event: &str, body: String, key: Option<&[u8]>) -> GitHubHookReq {
    GitHubHookReq {
        event: event.to_owned(),
        delivery: Some("d-1".to_owned()),
        signature: key.map(|k| sign(k, &body)),
        body,
    }
}

/// A rig whose controller holds [`SECRET`] and an App that reads [`Hooked::github`].
pub(super) struct Hooked {
    /// The rig.
    pub(super) rig: Rig,
    /// The GitHub the App reads pull requests back from.
    pub(super) github: PullsApi,
}

impl Deref for Hooked {
    type Target = Rig;

    fn deref(&self) -> &Rig {
        &self.rig
    }
}

pub(super) fn hooked() -> Hooked {
    let mut r = rig();
    let github = PullsApi::start();
    r.cx =
        r.cx.sharing(HookSecret::new(SECRET).unwrap())
            .sharing(github.api.client());
    r.answer_repo();
    Hooked { rig: r, github }
}

impl Hooked {
    /// Sets the fake GitHub to what `req`'s payload says: its pull request open, or closed on
    /// `closed`, at its head, in the base repo, by a person.
    fn agree(&self, req: &GitHubHookReq) {
        let Ok(p) = serde_json::from_str::<Value>(&req.body) else {
            return;
        };
        let (Some(number), Some(head), Some(repo)) = (
            p.get("number").and_then(Value::as_u64),
            p.pointer("/pull_request/head/sha").and_then(Value::as_str),
            p.pointer("/repository/full_name").and_then(Value::as_str),
        ) else {
            return;
        };
        let open = p.get("action").and_then(Value::as_str) != Some("closed");
        let pull = pull_json((open, false), (head, repo), ("someone", "User"));
        self.github.set_pull(number, pull);
    }

    /// Delivers `req` from the gate once GitHub agrees with it, handling its event on the spot
    /// and starting each claimed Box at once.
    pub(super) fn deliver(&self, req: &GitHubHookReq) -> Result<GitHubHookValue, CommandError> {
        self.agree(req);
        self.rig.deliver(req)
    }

    /// Like [`Hooked::deliver`], handing each claimed start to `start`.
    fn deliver_holding(
        &self,
        req: &GitHubHookReq,
        start: &dyn Fn(StartJob),
    ) -> Result<GitHubHookValue, CommandError> {
        self.agree(req);
        self.rig.deliver_holding(req, start)
    }

    /// Delivers a signed `action` on pull request `number` at `head`, which must be accepted.
    pub(super) fn signed(&self, action: &str, number: u64, head: &str) {
        let req = delivery(
            "pull_request",
            body(action, ("o/n", number), head),
            Some(SECRET),
        );
        assert_eq!(self.deliver(&req).unwrap(), GitHubHookValue::Accepted);
    }
}

/// The gate's front.
fn gate() -> Caller {
    Caller::Front("gate".to_owned())
}

impl Rig {
    /// Stores workflow `name` of the repo with the trigger `on`, enabled when `enabled`.
    pub(super) fn put_pr(&self, name: &str, on: bool, enabled: bool) -> WorkflowId {
        let mut def = WorkflowDef::new(
            WorkflowName::parse(name).unwrap(),
            Some(WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap()),
            vec![repo()],
            "check the pull request",
            (enabled, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.on_pull_request = on;
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }

    /// Stores a workflow of the repo with no cron and the pull request trigger on.
    pub(super) fn put_pr_only(&self, name: &str) -> WorkflowId {
        let mut def = WorkflowDef::new(
            WorkflowName::parse(name).unwrap(),
            None,
            vec![repo()],
            "check the pull request",
            (true, TimeoutMinutes::DEFAULT),
        )
        .unwrap();
        def.on_pull_request = true;
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }

    /// Delivers `req` from the gate, handling its event on the spot and starting each claimed
    /// Box at once.
    pub(super) fn deliver(&self, req: &GitHubHookReq) -> Result<GitHubHookValue, CommandError> {
        self.deliver_holding(req, &|job| {
            run_start(&self.cx, &job);
        })
    }

    /// Like [`Rig::deliver`], handing each claimed start to `start`.
    fn deliver_holding(
        &self,
        req: &GitHubHookReq,
        start: &dyn Fn(StartJob),
    ) -> Result<GitHubHookValue, CommandError> {
        let start = |job| {
            start(job);
            Ok(())
        };
        let handle = |pulled| {
            handle_with(&self.cx, req.delivery.as_deref(), &pulled, &start);
            Ok(())
        };
        check(&self.cx, &Ok(gate()), req, &handle)
    }

    /// The runs of `id` for pull request `number` at `head`.
    pub(super) fn runs_at(&self, id: WorkflowId, number: u64, head: &str) -> Vec<WorkflowRun> {
        self.runs(id)
            .into_iter()
            .filter(|r| {
                r.cause
                    .subject()
                    .is_some_and(|p| p.number == number && p.head_sha.as_str() == head)
            })
            .collect()
    }

    /// The one run of `id` for pull request `number` at `head`.
    pub(super) fn one_at(&self, id: WorkflowId, number: u64, head: &str) -> WorkflowRun {
        let mut found = self.runs_at(id, number, head);
        assert_eq!(found.len(), 1, "{found:?}");
        found.remove(0)
    }
}

pub(super) fn cancelled(run: &WorkflowRun) -> Option<&CancelCause> {
    match &run.state {
        RunState::Done(
            RunEnd::Ended {
                outcome: RunOutcome::Cancelled { cause, .. },
                ..
            }
            | RunEnd::CancelledUnstarted { cause },
        ) => Some(cause),
        _ => None,
    }
}

#[test]
fn a_signed_opened_fires_a_pull_request_run_with_its_head() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    let run = r.one_at(id, 65, OLD);
    assert!(
        matches!(run.state, RunState::Running { .. }),
        "{:?}",
        run.state
    );
    assert_eq!(run.cause.trigger().as_str(), "pull_request");
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_workflow_with_no_cron_still_fires_on_a_pull_request() {
    let r = hooked();
    let id = r.put_pr_only("evidence");
    r.signed("opened", 65, OLD);
    let run = r.one_at(id, 65, OLD);
    assert!(
        matches!(run.state, RunState::Running { .. }),
        "{:?}",
        run.state
    );
    assert_eq!(run.cause.trigger().as_str(), "pull_request");
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_bad_or_missing_signature_records_nothing() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    let b = body("opened", ("o/n", 65), OLD);
    let mut forged = delivery("pull_request", b.clone(), Some(b"not-the-secret"));
    for req in [forged.clone(), delivery("pull_request", b.clone(), None)] {
        assert_eq!(r.deliver(&req).unwrap(), GitHubHookValue::BadSignature);
    }
    forged.signature = Some(sign(SECRET, &b));
    forged.body = body("opened", ("o/n", 65), NEW);
    assert_eq!(
        r.deliver(&forged).unwrap(),
        GitHubHookValue::BadSignature,
        "signed another body"
    );
    let ping = delivery("ping", "{}".to_owned(), None);
    assert_eq!(r.deliver(&ping).unwrap(), GitHubHookValue::BadSignature);
    assert!(r.runs(id).is_empty());
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn only_a_front_with_a_secret_takes_deliveries() {
    let r = rig();
    let req = delivery("ping", "{}".to_owned(), Some(SECRET));
    let none = |_| Ok(());
    assert_eq!(
        check(&r.cx, &Ok(gate()), &req, &none).unwrap(),
        GitHubHookValue::Off
    );
    let r = hooked();
    let refused = check(&r.cx, &Ok(Caller::Runner), &req, &none).unwrap_err();
    assert!(
        refused.error.to_string().contains("only a front"),
        "{refused:?}"
    );
}

#[test]
fn ping_installation_and_other_actions_are_accepted_and_do_nothing() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    for (event, b) in [
        (
            "ping",
            json!({"zen": "Keep it logically awesome."}).to_string(),
        ),
        ("installation", json!({"action": "created"}).to_string()),
        ("pull_request", body("labeled", ("o/n", 65), OLD)),
        ("pull_request", body("edited", ("o/n", 65), OLD)),
        ("issues", json!({"action": "opened"}).to_string()),
    ] {
        let req = delivery(event, b, Some(SECRET));
        assert_eq!(
            r.deliver(&req).unwrap(),
            GitHubHookValue::Accepted,
            "{event}"
        );
    }
    assert!(r.runs(id).is_empty());
}

#[test]
fn a_signed_pull_request_body_that_does_not_parse_is_refused() {
    let r = hooked();
    let req = delivery(
        "pull_request",
        "{\"action\":\"opened\"}".to_owned(),
        Some(SECRET),
    );
    assert!(r.deliver(&req).is_err());
}

#[test]
fn reopened_ready_for_review_and_synchronize_fire_too() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    for (n, action) in [
        (70, "reopened"),
        (71, "ready_for_review"),
        (72, "synchronize"),
    ] {
        r.signed(action, n, OLD);
        assert!(
            matches!(r.one_at(id, n, OLD).state, RunState::Running { .. }),
            "{action}"
        );
    }
}

#[test]
fn only_enabled_workflows_with_the_trigger_on_fire_and_the_repo_matches_without_case() {
    let r = hooked();
    let on = r.put_pr("evidence", true, true);
    let off = r.put_pr("nightly", false, true);
    let disabled = r.put_pr("paused", true, false);
    let req = delivery(
        "pull_request",
        body("opened", ("O/N", 65), OLD),
        Some(SECRET),
    );
    assert_eq!(r.deliver(&req).unwrap(), GitHubHookValue::Accepted);
    assert_eq!(r.runs_at(on, 65, OLD).len(), 1);
    assert!(r.runs(off).is_empty());
    assert!(r.runs(disabled).is_empty());
    let other = delivery(
        "pull_request",
        body("opened", ("o/other", 65), OLD),
        Some(SECRET),
    );
    r.deliver(&other).unwrap();
    assert_eq!(r.runs(on).len(), 1, "another repo fires nothing");
}

#[test]
fn a_push_cancels_the_older_heads_run() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    let old_box = r.one_at(id, 65, OLD).state.box_id().unwrap();
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    let old = r.one_at(id, 65, OLD);
    let sha = moor_core::workflow::CommitSha::parse(NEW).unwrap();
    assert_eq!(
        cancelled(&old),
        Some(&CancelCause::NewerHead { sha }),
        "{:?}",
        old.state
    );
    assert_eq!(old.state.box_id(), Some(old_box));
    assert!(matches!(
        r.one_at(id, 65, NEW).state,
        RunState::Running { .. }
    ));
}

#[test]
fn the_same_head_delivered_again_claims_once_even_after_its_run_ended() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    r.signed("opened", 65, OLD);
    assert_eq!(r.runs(id).len(), 1);
    r.signed("closed", 65, OLD);
    r.signed("reopened", 65, OLD);
    assert_eq!(
        r.runs(id).len(),
        1,
        "a reopen at the same head fires nothing"
    );
    assert!(cancelled(&r.one_at(id, 65, OLD)).is_some());
}

#[test]
fn a_late_delivery_of_an_older_head_leaves_the_newer_run_be() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    r.signed("opened", 65, OLD);
    assert!(matches!(
        r.one_at(id, 65, NEW).state,
        RunState::Running { .. }
    ));
    assert_eq!(r.runs(id).len(), 2);
}

#[test]
fn closed_cancels_the_open_run_of_that_pull_request_only() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    r.signed("opened", 65, OLD);
    r.signed("opened", 66, OLD);
    r.signed("closed", 65, OLD);
    let run = r.one_at(id, 65, OLD);
    assert_eq!(
        cancelled(&run),
        Some(&CancelCause::Closed),
        "{:?}",
        run.state
    );
    assert!(matches!(
        r.one_at(id, 66, OLD).state,
        RunState::Running { .. }
    ));
    r.signed("closed", 65, OLD);
    assert_eq!(
        cancelled(&r.one_at(id, 65, OLD)),
        Some(&CancelCause::Closed)
    );
}

#[test]
fn closed_while_the_box_is_being_started_cancels_the_run_and_its_start_closes_the_box() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    let held = RefCell::new(Vec::new());
    let req = delivery(
        "pull_request",
        body("opened", ("o/n", 65), OLD),
        Some(SECRET),
    );
    r.deliver_holding(&req, &|job| held.borrow_mut().push(job))
        .unwrap();
    assert!(matches!(r.one_at(id, 65, OLD).state, RunState::Queued));
    r.signed("closed", 65, OLD);
    let run = r.one_at(id, 65, OLD);
    assert_eq!(
        run.state,
        RunState::Done(RunEnd::CancelledUnstarted {
            cause: CancelCause::Closed
        })
    );
    let job = held.borrow_mut().remove(0);
    let name: BoxName = job.firing.name.clone();
    run_start(&r.cx, &job);
    assert_eq!(
        r.one_at(id, 65, OLD).state,
        run.state,
        "the late start changes nothing"
    );
    let row = r.cx.store().unwrap().latest_box_by_name(&name).unwrap();
    let row = row.unwrap_or_else(|| panic!("no Box {}", name.as_str()));
    assert_eq!(
        lifecycle(&r, row.id),
        BoxLifecycle::Closed,
        "the late Box is closed"
    );
}

#[test]
fn a_push_while_the_older_box_is_being_started_cancels_it_and_fires_the_new_head() {
    let r = hooked();
    let id = r.put_pr("evidence", true, true);
    let held = RefCell::new(Vec::new());
    let req = delivery(
        "pull_request",
        body("opened", ("o/n", 65), OLD),
        Some(SECRET),
    );
    r.deliver_holding(&req, &|job| held.borrow_mut().push(job))
        .unwrap();
    r.pause(1_000);
    r.signed("synchronize", 65, NEW);
    assert!(matches!(
        r.one_at(id, 65, OLD).state,
        RunState::Done(RunEnd::CancelledUnstarted { .. })
    ));
    assert!(matches!(
        r.one_at(id, 65, NEW).state,
        RunState::Running { .. }
    ));
}
