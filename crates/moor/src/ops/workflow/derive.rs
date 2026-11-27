//! moor reads a workflow's prompt into a plan or a reading. A write that changes the prompt asks
//! for one; this pass starts an agent place that holds the workflow's repos, with no checkout and
//! no setup. Its agent reads the repos through the Proxy and either hands in a reading
//! ([`hand_in()`]) or leaves `plan.json`, which the pass stores when the prompt is still the one it
//! was made from, closing the place. Nothing else stores one. A plan clears the gates and the
//! reading held, and a reading clears the plan. Gates a person wrote stay until the owner asks
//! moor to make the reading again. A place that fails or ends without a reading leaves the reason:
//! its start or launch failure, or its agent's last line. moor tries again only when the prompt
//! changes or the owner asks: no try repeats on its own.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::close_reason::CloseReason;
use moor_core::domain::place::Claim;
use moor_core::name::BoxName;
use moor_core::request::ArtifactReadReq;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    draft_prompt, plan_box_name, PlanDraft, PromptHash, ReaderLeft, Workflow, WorkflowId, PLAN_FILE,
};

use super::close_open_box;
use crate::cx::Cx;
use crate::ops::{artifacts, place, show, start};
use crate::store::Store;

mod hand_in;
pub(crate) use hand_in::run as hand_in;

/// How long a Box asked for may go unseen before moor gives up on it.
const UNSEEN_MS: i64 = 10 * 60_000;
/// How long a Box may sit idle without a plan before moor gives up on it.
const QUIET_MS: i64 = 2 * 60_000;
/// The longest a Box may take to make a plan.
const LIMIT_MS: i64 = 30 * 60_000;
/// The largest plan read.
const PLAN_MAX: u64 = 256 * 1024;

/// An agent place to start that reads a workflow's prompt.
#[derive(Debug, Clone)]
pub(crate) struct DraftJob {
    /// The workflow.
    pub(crate) workflow: WorkflowId,
    /// Whose place it is.
    pub(crate) owner: UserId,
    /// An identity of the owner, which the start acts as.
    pub(crate) identity: Identity,
    /// The draft as stored once the place was asked for.
    pub(crate) draft: PlanDraft,
    /// The place's name, which the draft holds.
    pub(crate) name: BoxName,
    /// What the place holds: the workflow's repos and what its agent is asked.
    pub(crate) claim: Claim,
}

/// One pass over every workflow whose plan moor is making, handing each place to start to
/// `launch`, which must not wait for the start. Returns what to log.
pub(crate) fn pass(cx: &Cx, launch: &dyn Fn(DraftJob)) -> Vec<String> {
    let mut out = Vec::new();
    let drafts = cx
        .store()
        .map_err(|e| e.error.to_string())
        .and_then(|s| s.plan_drafts().map_err(|e| e.to_string()).map(|d| (s, d)));
    match drafts {
        Ok((store, drafts)) => {
            for (id, draft) in drafts {
                if let Err(why) = one(cx, store, (id, &draft), launch) {
                    out.push(format!("plan of workflow {id}: {why}"));
                }
            }
        }
        Err(why) => out.push(why),
    }
    out
}

/// Moves one draft on.
fn one(
    cx: &Cx,
    store: &Store,
    (id, draft): (WorkflowId, &PlanDraft),
    launch: &dyn Fn(DraftJob),
) -> Result<(), String> {
    let Some(w) = store.workflow_by_id(id).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if draft.failed.is_some() {
        return Ok(());
    }
    let now = cx.clock().now().as_millis();
    let by_hand = w.gates.as_ref().is_some_and(|g| g.made_from().is_none());
    if !draft.is_for(&w.prompt) || (by_hand && !draft.replace) {
        if let Some(name) = draft.box_name.as_deref() {
            close(cx, store, name);
        }
        let next = (!by_hand || draft.replace)
            .then(|| PlanDraft::wanted(&w.prompt).replacing(draft.replace));
        store
            .move_plan_draft(id, draft, next.as_ref())
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    draft.box_name.as_deref().map_or_else(
        || ask(store, &w, draft, (now, launch)),
        |name| harvest(cx, store, (&w, draft), name, now),
    )
}

/// Asks for the place that reads `w`'s prompt, recording it before it starts.
fn ask(
    store: &Store,
    w: &Workflow,
    draft: &PlanDraft,
    (now, launch): (i64, &dyn Fn(DraftJob)),
) -> Result<(), String> {
    let fail = |why: &str| {
        store
            .move_plan_draft(w.id, draft, Some(&draft.failed(why)))
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    let Some((repo, also)) = w.repos.split_first() else {
        return fail("the workflow has no repo");
    };
    let identity = store
        .identities(w.owner)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next();
    let Some(identity) = identity else {
        return fail("the owner has no identity to start a place as");
    };
    let hash = PromptHash::of(&w.prompt);
    let name = plan_box_name(w.id, w.name.as_str(), &hash, Ts::from_millis(now))
        .map_err(|e| e.to_string())?;
    let asked = draft.asked(name.as_str(), now);
    if !store
        .move_plan_draft(w.id, draft, Some(&asked))
        .map_err(|e| e.to_string())?
    {
        return Ok(());
    }
    launch(DraftJob {
        workflow: w.id,
        owner: w.owner,
        identity,
        draft: asked,
        name,
        claim: Claim {
            repo: repo.clone(),
            also: also.to_vec(),
            prompt: draft_prompt(&w.prompt, w.config.as_deref()),
            at: now,
            served: None,
            steered: None,
        },
    });
    Ok(())
}

/// Starts `job`'s place as the owner `cx` acts for, once the owner can read each repo; a start
/// that fails leaves its reason on the draft, where the workflow's page shows it.
pub(crate) fn start_draft(cx: &Cx, job: &DraftJob) {
    let Ok(store) = cx.store() else {
        return;
    };
    let started = job
        .claim
        .repos()
        .try_for_each(|r| start::repo::fetch(cx, job.owner, r, None).map(|_| ()))
        .and_then(|()| place::start_claimed(cx, store, job.name.clone(), job.claim.clone()));
    let Err(e) = started else {
        return;
    };
    let failed = job.draft.failed(format!(
        "the agent place that reads the prompt did not start: {}",
        e.error
    ));
    let _ = store.move_plan_draft(job.workflow, &job.draft, Some(&failed));
}

/// Reads the plan the place `name` left, stores it, and closes the place; or gives up on a place
/// that will leave none, saying why.
fn harvest(
    cx: &Cx,
    store: &Store,
    (w, draft): (&Workflow, &PlanDraft),
    name: &str,
    now: i64,
) -> Result<(), String> {
    let since = draft.since.unwrap_or(now);
    let fail = |why: String| {
        close(cx, store, name);
        store
            .move_plan_draft(w.id, draft, Some(&draft.failed(why)))
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    let box_name = BoxName::parse(name).map_err(|e| e.to_string())?;
    let Some(row) = store
        .open_box_by_name(&box_name)
        .map_err(|e| e.to_string())?
    else {
        if now - since > UNSEEN_MS {
            return fail(
                "the agent place that reads the prompt closed before it handed in a reading"
                    .to_owned(),
            );
        }
        return Ok(());
    };
    if let Some(text) = plan_text(cx, store, &row)? {
        return match ReaderLeft::parse(&text, &w.prompt) {
            Ok(ReaderLeft::Plan(plan)) => {
                close(cx, store, name);
                store
                    .store_made_plan(w.id, &plan)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            Ok(ReaderLeft::Gates(gates)) => {
                close(cx, store, name);
                hand_in::hold_gates(cx, store, (w, draft), &row, &gates)
                    .map_err(|e| e.error.to_string())
            }
            Err(errors) => fail(format!(
                "the plan the Box left is not valid: {}",
                errors.join("; ")
            )),
        };
    }
    if row.lifecycle == BoxLifecycle::SetupFailed || row.launch_failure.is_some() {
        return fail(format!(
            "the agent place that reads the prompt failed: {}",
            place::failure(&row)
        ));
    }
    let quiet = row
        .idle_since
        .is_some_and(|at| now - at.as_millis() > QUIET_MS);
    if quiet || now - since > LIMIT_MS {
        return fail(ended(store, &row)?);
    }
    Ok(())
}

/// Why the reader in place `row` ended without a reading: its agent's last line, when it said one.
fn ended(store: &Store, row: &crate::store::BoxRow) -> Result<String, String> {
    const ENDED: &str = "the reader finished without leaving a plan or handing in a reading";
    let said = show::said_of(store, row.id).map_err(|e| e.error.to_string())?;
    let last = said
        .as_ref()
        .and_then(|s| s.text.lines().rev().map(str::trim).find(|l| !l.is_empty()));
    Ok(last.map_or_else(
        || format!("{ENDED}, and said nothing"),
        |line| format!("{ENDED}; its last line: {line}"),
    ))
}

/// The newest `plan.json` the place `row` left, as text; none while it has left none.
fn plan_text(cx: &Cx, store: &Store, row: &crate::store::BoxRow) -> Result<Option<String>, String> {
    let left = store
        .latest_artifacts(row.id)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|a| a.name == PLAN_FILE);
    if !left {
        return Ok(None);
    }
    let req = ArtifactReadReq {
        target: row.name.clone().into(),
        name: PLAN_FILE.to_owned(),
        version: None,
        offset: 0,
        len: PLAN_MAX,
        everyone: true,
    };
    let mut bytes = Vec::new();
    artifacts::read(cx, &req, &mut |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })
    .map_err(|e| e.error.to_string())?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "the plan the Box left is not text".to_owned())
}

/// Closes the Box `name` when it is open; a close that fails is tried again by the Box's own
/// idle close.
fn close(cx: &Cx, store: &Store, name: &str) {
    if let Ok(name) = BoxName::parse(name) {
        let _ = close_open_box(cx, store, &name, CloseReason::RunEnded);
    }
}
