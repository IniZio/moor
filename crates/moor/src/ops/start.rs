//! `start` (minimal): check credentials, create the Box and its Workspace, acquire the machine.
//!
//! It queues the Box when the machine limit for its provider is reached. On a new machine it runs the repo
//! setup, then the dotfiles, before the agent launches.
//! It launches the agent for a Working Box; without a prompt or issue the agent waits for a prompt.

use moor_core::config::{Flags, RepoConfig};
use moor_core::decide::schedule::BoxSpec;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{BoxRepo, Decision, ExtraEvent, NewBox, SpecFields};
use moor_core::domain::event::CheckedCred;
use moor_core::domain::hands::HandsState;
use moor_core::domain::inbox::InboxKind;
use moor_core::name::NodeName;
use moor_core::seam::forge::RepoFiles;
use moor_core::user::UserId;
use moor_core::view::StartView;
use moor_core::{BoxId, ErrorKind, SessionId, StartReq, Warning};

use super::common::activity;
use super::common::autosleep;
use super::common::facts::{repo_config_of, resolved, settings_of};
use super::common::node;
use super::common::promote::promote_waiting;
use super::common::provision::{provision, reprovision, reread, Entry};
use super::common::provisioning::dead_provisioning;
use super::common::reconcile;
use super::common::wake::{resume_owed, Seen};
use super::host_session::refuse_invalid;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store, StoreError};

mod asleep;
mod cold;
mod launch;
mod needs;
mod preflight;
pub(crate) mod profile;
mod profile_env;
mod relaunch;
mod reopen;
pub(crate) mod repo;
mod response;
mod restore;
mod revive;
mod watch;

use response::{queued_view, set_agent, set_provisioned, view};

pub(crate) use launch::{begin_seeded, queued_or};
pub(crate) use needs::run as needs;
pub(crate) use revive::{
    capped, launch_after_retry, revivable, revive_after_wake, revive_for_steer, AfterWake,
};
pub(crate) use watch::run as watch_launch;

/// What `start` learns before it changes anything.
struct Facts {
    /// The repo the Box clones, with its config and author at the base; `None` for no repo.
    repo: Option<BoxRepo>,
    /// The full sha the Box starts from; `None` for a Box with no repo.
    base: Option<String>,
    /// The repo's MCP and Claude settings files at the base.
    files: RepoFiles,
    /// The requester's agent profile; `None` when they set none.
    profile: Option<profile::Gathered>,
}

/// Starts or attaches to the Box named in `req`.
///
/// # Errors
/// [`CommandError`] when a read, a seam call or a store write fails.
pub(crate) fn run(cx: &Cx, req: &StartReq) -> Result<StartView, CommandError> {
    let store = cx.store()?;
    let user = needs::require(cx, req.repo.as_ref())?;
    if let Some(parent) = req.parent {
        store.check_parent(parent.get()).map_err(refuse_invalid)?;
    }
    settle_all(cx, store, &|| false)?;
    if let Some(row) = store.open_box_by_name(&req.name)? {
        cold::warn_ignored(cx, req);
        return attach(cx, store, req, &row, user);
    }
    if req.resume {
        return Err(relaunch::nothing_to_resume());
    }
    let mut facts = gather(cx, req, user)?;
    let flags = Flags {
        model: req.model.clone(),
    };
    let repo_cfg = repo_config_of(facts.repo.as_ref(), facts.base.as_deref())?;
    let cfg = resolved(cx, repo_cfg.as_ref(), &flags);
    let spec = spec_of(req, repo_cfg.as_ref())?;
    let note = node::preflight(cx, user, &spec)?;
    if let Some(reason) = preflight::rejection(cx, &cfg, &facts.files, user)? {
        return reject(cx, store, (req, user), &facts, &reason);
    }
    let new = new_box(req, &facts, spec.labels.clone());
    let id = match insert(store, (req, user), new, preflight::checked(&cfg)) {
        Ok(id) => id,
        Err(e) => return raced(cx, store, (req, user), e),
    };
    let turn = cx.turn(id, Cmd::Start)?;
    queue_prompt(store, id, req)?;
    store.set_box_node(id, spec.node.as_ref())?;
    cold::record(store, req, id)?;
    if let Some(gathered) = facts.profile.take() {
        profile::record(cx, store, id, gathered)?;
    }
    let done = provision(cx, store, id, Entry::Fresh, &cfg)?;
    let agent = launch::run(cx, store, req, id, done.setup.exit(), &|| Ok(cfg.clone()))?;
    drop(turn);
    let agent = watch::run(cx, store, id, agent);
    let mut v = settled(cx, store, id, note)?;
    set_provisioned(&mut v, &done);
    set_agent(&mut v, agent);
    Ok(v)
}

/// The view of a start that has run, naming the personal node it started on, once the Boxes
/// pending on its node have had their turn.
fn settled(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    note: Option<String>,
) -> Result<StartView, CommandError> {
    if let Some(on) = store.box_spec(id)?.node {
        promote_waiting(cx, store, &on);
    }
    let row = reread(store, id)?;
    let mut v = queued_view(cx, &row, false);
    if row.lifecycle != BoxLifecycle::Pending {
        v.warnings_inline.extend(note);
    }
    Ok(v)
}

/// What the start asks of a node: the labels its repo requires, the node the requester named,
/// and the node the requester named.
fn spec_of(req: &StartReq, repo: Option<&RepoConfig>) -> Result<BoxSpec, CommandError> {
    let node = req
        .node
        .as_deref()
        .map(NodeName::parse)
        .transpose()
        .map_err(|e| CommandError::new(ErrorKind::Refused, e.to_string()))?;
    Ok(BoxSpec {
        labels: repo.map(|r| r.node.labels.clone()).unwrap_or_default(),
        node,
    })
}

/// Records a rejected Box, with no Workspace, and fails with the outcome.
fn reject(
    cx: &Cx,
    store: &Store,
    (req, user): (&StartReq, UserId),
    facts: &Facts,
    reason: &str,
) -> Result<StartView, CommandError> {
    let id = store
        .apply(
            Decision::insert(new_box(req, facts, Vec::new()))
                .parent_session(req.parent.map(SessionId::get))
                .requester(user),
        )?
        .box_id;
    let rejected = BoxLifecycleInput::Rejected {
        reason: reason.to_owned(),
    };
    store.apply(Decision::new(id).lifecycle(rejected))?;
    let row = reread(store, id)?;
    let shown = serde_json::to_value(view(cx, &row, false))
        .map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    Err(CommandError::new(ErrorKind::Failed, reason.to_owned()).with_value(shown))
}

/// Another start made the Box first: attach to it, else report `lost`.
fn raced(
    cx: &Cx,
    store: &Store,
    (req, user): (&StartReq, UserId),
    lost: impl Into<CommandError>,
) -> Result<StartView, CommandError> {
    let open = store.open_box_by_name(&req.name)?;
    open.map_or_else(
        || Err(lost.into()),
        |row| attach(cx, store, req, &row, user),
    )
}

/// Starts over on an open Box whose machine is lost; otherwise only attaches. A Box that acts
/// for another user is refused.
fn attach(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    row: &BoxRow,
    user: UserId,
) -> Result<StartView, CommandError> {
    preflight::check_owner(row, user)?;
    repo::check_kept(req, row)?;
    relaunch::check_resume(req, row)?;
    dead_provisioning(cx, row)?;
    reconcile::one(cx, store, row);
    let turn = cx.turn(row.id, Cmd::Start)?;
    queue_prompt(store, row.id, req)?;
    let row = &reread(store, row.id)?;
    let reprovisioned = reprovision(cx, store, row, &turn)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::Attached))?;
    let setup_exit = reprovisioned.as_ref().and_then(|p| p.setup.exit());
    let settings = || {
        settings_of(
            cx,
            row,
            &Flags {
                model: req.model.clone(),
            },
        )
    };
    if asleep::wake(cx, store, req, row.id)? == asleep::Woken::Queued {
        return Ok(queued_view(cx, &reread(store, row.id)?, true));
    }
    if relaunch::asks_for_agent(req) {
        reopen::ended_session(store, row.id, req.parent.map(SessionId::get))?;
    }
    asleep::wake_idle(store, row.id)?;
    let agent = relaunch::run(cx, store, req, row.id, setup_exit, &settings)?;
    drop(turn);
    let agent = watch::run(cx, store, row.id, agent);
    let _turn = cx.turn(row.id, Cmd::Steer)?;
    crate::ops::steer::deliver(cx, store, row.id)?;
    let row = &reread(store, row.id)?;
    let mut v = queued_view(cx, row, true);
    if let Some(p) = reprovisioned {
        set_provisioned(&mut v, &p);
    }
    set_agent(&mut v, agent);
    Ok(v)
}

/// Queues the prompt `req` gives for Box `id`'s agent, once the start holds the Box's turn, so a
/// start refused before then leaves nothing queued. The same prompt already waiting is not queued
/// again: a start retried with its prompt asks for it once. A fresh launch carries it, or it is typed in
/// once the agent can take it.
fn queue_prompt(store: &Store, id: BoxId, req: &StartReq) -> Result<(), CommandError> {
    let Some(prompt) = &req.prompt else {
        return Ok(());
    };
    let waits = store
        .unsent(id)?
        .iter()
        .any(|r| r.kind == InboxKind::Prompt && r.text == *prompt && r.delivery.launch().is_none());
    if !waits {
        store.enqueue(id, InboxKind::Prompt, prompt, InboxKind::Prompt.as_str())?;
    }
    Ok(())
}

/// Moves each open Box with a ready machine between Working and Idle, until `draining`.
///
/// Runs before the start, which may then leave the Box it starts in another state.
pub(crate) fn settle_all(
    cx: &Cx,
    store: &Store,
    draining: &dyn Fn() -> bool,
) -> Result<(), CommandError> {
    for row in &store.open_boxes()? {
        if draining() {
            break;
        }
        if row.hands.as_ref().map(|b| b.state) == Some(HandsState::Ready) {
            cx.for_box(row.user, || settle_one(cx, store, row));
        }
    }
    Ok(())
}

/// Moves one open Box with a ready machine between Working and Idle, then lets it auto-sleep.
fn settle_one(cx: &Cx, store: &Store, row: &BoxRow) {
    let settled = activity::read(cx, store, row, cx.clock().now(), activity::PROBE_DEADLINE)
        .and_then(|reading| activity::settle(cx, store, row, &reading).map(|f| (reading, f)));
    match settled {
        Ok((reading, f)) => {
            if let Err(e) = resume_owed(cx, store, row.id, Seen::of(&reading, f)) {
                cx.warn(Warning {
                    code: "settle_failed".to_owned(),
                    message: format!("could not resume Box {}: {}", row.id.get(), e.error),
                });
            }
            autosleep::run(cx, store, row.id, autosleep::AgentRead::of(&reading));
        }
        Err(e) => cx.warn(Warning {
            code: "settle_failed".to_owned(),
            message: format!("could not settle Box {}: {}", row.id.get(), e.error),
        }),
    }
}

/// Reads the repo the Box starts from through the forge; changes nothing.
fn gather(cx: &Cx, req: &StartReq, user: UserId) -> Result<Facts, CommandError> {
    let profile = profile::gather(cx, user)?;
    repo::check_first(req)?;
    let Some(repo) = &req.repo else {
        return Ok(Facts {
            repo: None,
            base: None,
            files: RepoFiles::default(),
            profile,
        });
    };
    let at = repo::read(cx, user, repo, req.base.as_deref())?;
    let also = req.also.as_deref().unwrap_or(&cx.config().box_.also);
    let others = repo::others(cx, user, repo, also)?;
    Ok(Facts {
        repo: Some(BoxRepo {
            repo: repo.clone(),
            moor_yaml: at.files.moor_yaml.clone(),
            author: at.author,
            others,
        }),
        base: Some(at.base.as_str().to_owned()),
        files: at.files,
        profile,
    })
}

/// The Box `req` starts, asking its node for `labels`.
fn new_box(req: &StartReq, facts: &Facts, labels: Vec<String>) -> NewBox {
    NewBox {
        name: req.name.clone(),
        repo: facts.repo.clone(),
        spec: SpecFields {
            issue: req.issue.clone(),
            prompt: req.prompt.clone(),
            base: facts.base.clone(),
            branch: None,
            labels,
        },
    }
}

/// Inserts the Box row with everything known before the machine.
fn insert(
    store: &Store,
    (req, user): (&StartReq, UserId),
    new: NewBox,
    checked: Vec<CheckedCred>,
) -> Result<BoxId, StoreError> {
    let mut decision = Decision::insert(new)
        .branch_by_id()
        .parent_session(req.parent.map(SessionId::get))
        .requester(user);
    if !checked.is_empty() {
        decision = decision.event(ExtraEvent::Checked {
            credentials: checked,
        });
    }
    Ok(store.apply(decision)?.box_id)
}

#[cfg(test)]
mod tests;
