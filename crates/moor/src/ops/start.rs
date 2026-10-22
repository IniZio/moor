//! `start` (minimal): check credentials, create the Box and its Workspace, acquire the machine.
//!
//! It queues the Box when the machine limit for its provider is reached. On a new machine it runs the repo
//! setup, then the dotfiles, before the agent launches.
//! It launches the agent for a Working Box with a goal.

use moor_core::config::Flags;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{BoxRepo, Decision, ExtraEvent, Field, Fields, NewBox};
use moor_core::domain::event::CheckedCred;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::forge::RepoFiles;
use moor_core::seam::surface::workspace_label;
use moor_core::user::UserId;
use moor_core::view::StartView;
use moor_core::{BoxId, ErrorKind, SessionId, StartReq, Warning};

use super::common::activity;
use super::common::autosleep;
use super::common::boxlock::{self, BoxLock};
use super::common::facts::{
    provider_caps, repo_config_of, require_reverse_tunnel, resolved, settings_of,
};
use super::common::provision::{provision, reprovision, reread, Entry, Provisioned};
use super::common::provisioning::dead_provisioning;
use super::host_session::refuse_invalid;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store, StoreError};

mod launch;
mod needs;
mod preflight;
mod relaunch;
mod reopen;
pub(crate) mod repo;
mod response;
mod watch;

use response::{queued_view, set_agent, set_provisioned, view, with_provider};

pub(crate) use launch::begin_seeded;
pub(crate) use needs::run as needs;
pub(crate) use relaunch::revive_after_wake;
pub(crate) use watch::run as watch_launch;

/// What `start` learns before it changes anything.
struct Facts {
    /// The repo the Box clones, with its config and author at the base; `None` for no repo.
    repo: Option<BoxRepo>,
    /// The full sha the Box starts from; `None` for a Box with no repo.
    base: Option<String>,
    /// The repo's MCP and Claude settings files at the base.
    files: RepoFiles,
}

/// Where the Box works.
struct Placed {
    /// The branch the Box works on.
    branch: String,
    /// Workspace id.
    workspace: String,
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
    settle_all(cx, store)?;
    if let Some(row) = store.open_box_by_name(&req.name)? {
        return attach(cx, store, req, &row, user);
    }
    if req.resume {
        return Err(relaunch::nothing_to_resume());
    }
    let facts = gather(cx, req, user)?;
    let flags = Flags {
        model: req.model.clone(),
    };
    let repo_cfg = repo_config_of(facts.repo.as_ref(), facts.base.as_deref())?;
    let cfg = resolved(cx, repo_cfg.as_ref(), &flags);
    let caps = provider_caps(cx, &cfg)?;
    require_reverse_tunnel(&cfg, caps)?;
    if let Some(reason) = preflight::rejection(cx, &cfg, &facts.files, user)? {
        return reject(cx, store, (req, user), &facts, &reason);
    }
    let placed = match place(cx, req) {
        Ok(placed) => placed,
        Err(e) => return raced(cx, store, (req, user), e),
    };
    let id = match insert(
        store,
        (req, user),
        &facts,
        &placed,
        preflight::checked(&cfg),
    ) {
        Ok(id) => id,
        Err(e) => return raced(cx, store, (req, user), e),
    };
    let (done, agent) = {
        let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
        let done = if reread(store, id)?.boxed.is_none() {
            provision(cx, store, id, Entry::Fresh, &cfg)?
        } else {
            Provisioned::skipped()
        };
        let agent = launch::run(cx, store, req, id, done.setup.exit(), &|| Ok(cfg.clone()))?;
        (done, agent)
    };
    let agent = watch::run(cx, store, id, agent);
    let row = reread(store, id)?;
    let mut v = queued_view(cx, &row, false, &cfg.provider.value);
    set_provisioned(&mut v, &done);
    set_agent(&mut v, agent);
    Ok(v)
}

/// Records a rejected Box, with no Workspace, and fails with the outcome.
fn reject(
    cx: &Cx,
    store: &Store,
    (req, user): (&StartReq, UserId),
    facts: &Facts,
    reason: &str,
) -> Result<StartView, CommandError> {
    let set = Fields {
        issue: set_some(req.issue.clone()),
        goal: set_some(req.goal.clone()),
        base: set_some(facts.base.clone()),
        ..Fields::default()
    };
    let new = NewBox {
        name: req.name.clone(),
        repo: facts.repo.clone(),
    };
    let id = store
        .apply(
            Decision::insert(new)
                .set(set)
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
    relaunch::check_resume(req, row)?;
    dead_provisioning(cx, row)?;
    let reprovisioned = reprovision(cx, store, row, "start")?;
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
    let recreated = matches!(
        row.boxed.as_ref().map(|b| b.state),
        Some(BoxState::Failed | BoxState::Gone)
    );
    if !req.no_agent && (req.goal.is_some() || req.resume) {
        reopen::ended_session(store, row.id, req.parent.map(SessionId::get))?;
    }
    wake_idle(store, row.id)?;
    let agent = relaunch::run(cx, store, req, row.id, (setup_exit, recreated), &settings)?;
    let agent = watch::run(cx, store, row.id, agent);
    let row = &reread(store, row.id)?;
    let mut v = with_provider(cx, row, true)?;
    if let Some(p) = reprovisioned {
        set_provisioned(&mut v, &p);
    }
    set_agent(&mut v, agent);
    Ok(v)
}

/// Moves an Idle Box being started to Working, so the launch check sees a Working Box.
fn wake_idle(store: &Store, id: BoxId) -> Result<(), CommandError> {
    let row = reread(store, id)?;
    if !(row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()) {
        return Ok(());
    }
    match store.apply(Decision::new(id).session(SessionInput::AgentBusy)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Moves each open Box with a ready machine between Working and Idle.
///
/// Runs before the start, which may then leave the Box it starts in another state.
pub(crate) fn settle_all(cx: &Cx, store: &Store) -> Result<(), CommandError> {
    for row in &store.open_boxes()? {
        if row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready) {
            cx.for_box(row.user, || settle_one(cx, store, row));
        }
    }
    Ok(())
}

/// Moves one open Box with a ready machine between Working and Idle, then lets it auto-sleep.
fn settle_one(cx: &Cx, store: &Store, row: &BoxRow) {
    let settled = activity::read(cx, store, row, cx.clock().now(), activity::PROBE_DEADLINE)
        .and_then(|reading| activity::settle(cx, store, row, &reading));
    match settled {
        Ok(()) => autosleep::run(cx, store, row.id),
        Err(e) => cx.warn(Warning {
            code: "settle_failed".to_owned(),
            message: format!("could not settle Box {}: {}", row.id.get(), e.error),
        }),
    }
}

/// Reads the repo the Box starts from through the forge; changes nothing.
fn gather(cx: &Cx, req: &StartReq, user: UserId) -> Result<Facts, CommandError> {
    let Some(repo) = &req.repo else {
        return Ok(Facts {
            repo: None,
            base: None,
            files: RepoFiles::default(),
        });
    };
    let branch = moor_core::box_key::branch(&req.name);
    let at = repo::read(cx, user, repo, req.base.as_deref(), &branch)?;
    Ok(Facts {
        repo: Some(BoxRepo {
            repo: repo.clone(),
            moor_yaml: at.files.moor_yaml.clone(),
            author: at.author,
        }),
        base: Some(at.base.as_str().to_owned()),
        files: at.files,
    })
}

/// Names the Box's branch, refusing one an open Box already works on, then ensures the Workspace.
fn place(cx: &Cx, req: &StartReq) -> Result<Placed, CommandError> {
    let branch = moor_core::box_key::branch(&req.name);
    let owner = cx
        .store()?
        .open_boxes()?
        .into_iter()
        .find(|t| t.branch.as_deref() == Some(&branch));
    if let Some(t) = owner {
        return Err(CommandError::new(
            ErrorKind::Failed,
            format!(
                "the branch {branch} belongs to open Box {}",
                t.name.as_str()
            ),
        )
        .with_hint("pick another Box name"));
    }
    let ws = cx.surface().ensure_workspace(&workspace_label(&req.name))?;
    Ok(Placed {
        branch,
        workspace: ws.as_str().to_owned(),
    })
}

/// `Set` for a present value, `Keep` otherwise.
fn set_some<T>(v: Option<T>) -> Field<T> {
    v.map_or(Field::Keep, Field::Set)
}

/// Inserts the Box row with everything known before the machine.
fn insert(
    store: &Store,
    (req, user): (&StartReq, UserId),
    facts: &Facts,
    placed: &Placed,
    checked: Vec<CheckedCred>,
) -> Result<BoxId, StoreError> {
    let set = Fields {
        issue: set_some(req.issue.clone()),
        goal: set_some(req.goal.clone()),
        base: set_some(facts.base.clone()),
        branch: Field::Set(placed.branch.clone()),
        workspace_id: Field::Set(placed.workspace.clone()),
        ..Fields::default()
    };
    let new = NewBox {
        name: req.name.clone(),
        repo: facts.repo.clone(),
    };
    let mut decision = Decision::insert(new)
        .set(set)
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
