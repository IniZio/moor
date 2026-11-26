//! moor reads a workflow's prompt into a plan or a reading. A write that changes the prompt asks
//! for one; this pass starts a short Box that reads the repos and either hands in a reading
//! ([`hand_in()`]) or leaves `plan.json`, which the pass stores when the prompt is still the one it
//! was made from, closing the Box. Nothing else stores one. A plan clears the gates and the
//! reading held, and a reading clears the plan. Gates a person
//! wrote stay until the owner asks moor to make the reading again. A Box that ends without a
//! reading leaves the reason, and moor tries again only when the prompt changes or the owner asks:
//! no try repeats on its own.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::event::close_reason::CloseReason;
use moor_core::name::BoxName;
use moor_core::request::ArtifactReadReq;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    draft_prompt, plan_box_name, PlanDraft, PromptHash, ReaderLeft, Workflow, WorkflowId, PLAN_FILE,
};
use moor_core::StartReq;

use super::close_open_box;
use crate::cx::Cx;
use crate::ops::{artifacts, start};
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

/// A Box to start that makes a workflow's plan.
#[derive(Debug, Clone)]
pub(crate) struct DraftJob {
    /// The workflow.
    pub(crate) workflow: WorkflowId,
    /// Whose Box it is.
    pub(crate) owner: UserId,
    /// An identity of the owner, which the start acts as.
    pub(crate) identity: Identity,
    /// The draft as stored once the Box was asked for.
    pub(crate) draft: PlanDraft,
    /// The start.
    pub(crate) req: StartReq,
}

/// One pass over every workflow whose plan moor is making, handing each Box to start to
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

/// Asks for the Box that makes `w`'s plan, recording it before it starts.
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
        return fail("the owner has no identity to start a Box as");
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
        req: StartReq {
            placement: moor_core::decide::schedule::Placement::FirstOnline,
            config: w.config.clone(),
            prompt: Some(draft_prompt(&w.prompt, w.config.as_deref())),
            repo: Some(repo.clone()),
            also: Some(also.to_vec()),
            ..StartReq::named(name)
        },
    });
    Ok(())
}

/// Starts `job`'s Box as the owner `cx` acts for; a start that fails leaves its reason on the
/// draft, where the workflow's page shows it.
pub(crate) fn start_draft(cx: &Cx, job: &DraftJob) {
    let Err(e) = start::run(cx, &job.req) else {
        return;
    };
    let failed = job.draft.failed(format!(
        "the Box that makes the plan did not start: {}",
        e.error
    ));
    if let Ok(store) = cx.store() {
        let _ = store.move_plan_draft(job.workflow, &job.draft, Some(&failed));
    }
}

/// Reads the plan the Box `name` left, stores it, and closes the Box; or gives up on a Box that
/// will leave none.
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
            return fail("the Box that makes the plan closed before it left one".to_owned());
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
    let quiet = row
        .idle_since
        .is_some_and(|at| now - at.as_millis() > QUIET_MS);
    let broken = matches!(row.lifecycle, BoxLifecycle::SetupFailed);
    if quiet || broken || now - since > LIMIT_MS {
        return fail("the Box finished without leaving a plan".to_owned());
    }
    Ok(())
}

/// The newest `plan.json` the Box `row` left, as text; none while it has left none.
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
