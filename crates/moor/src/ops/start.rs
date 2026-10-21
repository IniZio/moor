//! `start` (minimal): check credentials, create the Box, its worktree and Workspace, acquire the machine.
//!
//! It queues the Box when the machine limit for its provider is reached. On a new machine it runs the repo
//! setup, then the dotfiles, before the agent launches.
//! It launches the agent for a Working Box with a goal.

use std::path::{Path, PathBuf};

use moor_core::config::Flags;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewBox};
use moor_core::domain::event::CheckedCred;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::surface::{workspace_label, Worktree};
use moor_core::user::UserId;
use moor_core::view::StartView;
use moor_core::{BoxId, ErrorKind, SessionId, StartReq, Warning};

use super::common::activity;
use super::common::autosleep;
use super::common::boxlock::{self, BoxLock};
use super::common::facts::{provider_caps, require_reverse_tunnel, resolved, settings};
use super::common::provision::{
    moor_yaml, provision, reprovision, reread, Entry, Provisioned, ResyncGates,
};
use super::common::provisioning::dead_provisioning;
use super::host_session::refuse_invalid;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store, StoreError};

mod base;
mod known_repo;
mod launch;
mod leftover;
mod preflight;
mod relaunch;
mod reopen;
mod response;
mod watch;

use response::{queued_view, set_agent, set_provisioned, view, with_provider};

pub(crate) use launch::begin_seeded;
pub(crate) use relaunch::revive_after_wake;
pub(crate) use watch::run as watch_launch;

/// What `start` learns before it changes anything.
struct Facts {
    /// Repo root of the invocation directory.
    repo: PathBuf,
    /// The user's worktree, when `--worktree` names one.
    bound: Option<PathBuf>,
    /// Base ref or the HEAD commit it starts from.
    base: String,
    /// Ref the new worktree branches from; `None` leaves it to the Surface.
    from: Option<String>,
}

/// Where the Box works.
struct Placed {
    /// Worktree and its branch.
    wt: Worktree,
    /// Workspace id.
    workspace: String,
}

/// Starts or attaches to the Box named in `req`.
///
/// # Errors
/// [`CommandError`] when a read, a seam call or a store write fails.
pub(crate) fn run(cx: &Cx, req: &StartReq) -> Result<StartView, CommandError> {
    let store = cx.store()?;
    let user = cx.requester()?;
    let (repo, bound, req) = locate(cx, req)?;
    let req = &req;
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
    let facts = gather(cx, req, repo, bound)?;
    let flags = Flags {
        model: req.model.clone(),
    };
    let cfg = match facts.bound.as_deref() {
        Some(worktree) => settings(cx, worktree, &flags)?,
        None => base::settings_at_base(cx, &facts.repo, &facts.base, &flags)?,
    };
    let caps = provider_caps(cx, &cfg)?;
    require_reverse_tunnel(&cfg, caps)?;
    if let Some(reason) =
        preflight::rejection(cx, &cfg, facts.bound.as_deref().unwrap_or(&facts.repo))
    {
        return reject(cx, store, (req, user), &facts, &reason);
    }
    let placed = match place(cx, req, &facts) {
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

/// Records a rejected Box, with no worktree or Workspace, and fails with the outcome.
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
        base: Field::Set(facts.base.clone()),
        bound: Field::Set(facts.bound.is_some()),
        ..Fields::default()
    };
    let new = NewBox {
        name: req.name.clone(),
        repo: facts.repo.display().to_string(),
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
    preflight::check_bound(cx, req, row)?;
    relaunch::check_resume(req, row)?;
    dead_provisioning(cx, row)?;
    if let Err(e) = cx
        .git()
        .exclude_scratch(Path::new(row.worktree.as_deref().unwrap_or(&row.repo)))
    {
        cx.warn(Warning {
            code: "exclude_failed".to_owned(),
            message: format!("could not hide scratch dirs from git: {e}"),
        });
    }
    let launching = || relaunch::will_launch(cx, store, req, row);
    let agent_live = || relaunch::agent_live(cx, store, row);
    let gates = ResyncGates {
        launching: &launching,
        agent_live: &agent_live,
    };
    let reprovisioned = reprovision(cx, store, row, "start", req.trust_moor_yaml, &gates)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::Attached))?;
    let setup_exit = reprovisioned.as_ref().and_then(|p| p.setup.exit());
    let settings = || {
        let repo = moor_yaml::guard(cx, row, req.trust_moor_yaml)?;
        Ok(resolved(
            cx,
            repo.as_ref(),
            &Flags {
                model: req.model.clone(),
            },
        ))
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
        let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
        if !ready {
            continue;
        }
        let settled = activity::read(cx, store, row, cx.clock().now())
            .and_then(|reading| activity::settle(cx, store, row, &reading));
        if let Err(e) = settled {
            cx.warn(Warning {
                code: "settle_failed".to_owned(),
                message: format!("could not settle Box {}: {}", row.id.get(), e.error),
            });
            continue;
        }
        autosleep::run(cx, store, row.id);
    }
    Ok(())
}

/// An absolute form of `path`, relative to the invocation directory.
fn resolve(cx: &Cx, path: &str) -> PathBuf {
    cx.cwd().join(path)
}

/// The repo root, the bound worktree and the effective request; refuses bad input before any change.
fn locate(cx: &Cx, req: &StartReq) -> Result<(PathBuf, Option<PathBuf>, StartReq), CommandError> {
    if let Some(key) = &req.repo {
        let (repo, eff) = known_repo::prepare(cx, req, key)?;
        return Ok((repo, None, eff));
    }
    let git = |e: crate::adapters::git::GitError| CommandError::new(ErrorKind::Failed, e);
    let repo = cx.git().toplevel(cx.cwd()).map_err(git)?;
    let bound = req.worktree.as_deref().map(|w| resolve(cx, w));
    if let Some(wt) = &bound {
        let same = |a: &Path, b: &Path| {
            a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
        };
        if same(wt, &repo) {
            return Err(CommandError::new(
                ErrorKind::Usage,
                format!("{} is the main worktree, not a linked one", wt.display()),
            ));
        }
        cx.git().head(wt).map_err(|_| {
            CommandError::new(
                ErrorKind::Usage,
                format!("{} is not a git worktree", wt.display()),
            )
        })?;
    }
    Ok((repo, bound, req.clone()))
}

/// Reads the base the Box starts from; changes nothing.
fn gather(
    cx: &Cx,
    req: &StartReq,
    repo: PathBuf,
    bound: Option<PathBuf>,
) -> Result<Facts, CommandError> {
    let (base, from) = base::resolve(cx, req, &repo, bound.as_deref())?;
    Ok(Facts {
        repo,
        bound,
        base,
        from,
    })
}

/// Hides scratch dirs from git, creates the worktree unless one is bound, then ensures the
/// Workspace.
///
/// The exclude file lives in the common git dir, so hiding from a bound worktree or the repo covers
/// every worktree.
/// Doing it first means a failure leaves no worktree without a Box row.
fn place(cx: &Cx, req: &StartReq, facts: &Facts) -> Result<Placed, CommandError> {
    cx.git()
        .exclude_scratch(facts.bound.as_deref().unwrap_or(&facts.repo))
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    let wt = match &facts.bound {
        Some(path) => Worktree {
            path: path.clone(),
            branch: String::new(),
        },
        None => leftover::create_worktree(cx, req, facts)?,
    };
    let ws = cx
        .surface()
        .ensure_workspace(&wt, &workspace_label(&req.name))?;
    Ok(Placed {
        wt,
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
    let branch = Some(placed.wt.branch.clone()).filter(|b| !b.is_empty());
    let set = Fields {
        issue: set_some(req.issue.clone()),
        goal: set_some(req.goal.clone()),
        base: Field::Set(facts.base.clone()),
        branch: set_some(branch),
        worktree: Field::Set(placed.wt.path.display().to_string()),
        bound: Field::Set(facts.bound.is_some()),
        workspace_id: Field::Set(placed.workspace.clone()),
        ..Fields::default()
    };
    let new = NewBox {
        name: req.name.clone(),
        repo: facts.repo.display().to_string(),
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
