//! `start` (minimal): check credentials, create the Task, its worktree and Workspace, acquire the Box.
//!
//! It queues the Task when the box limit for its provider is reached. On a new Box it runs the repo
//! setup, then the dotfiles, before the agent launches.
//! It launches the agent for a Working Task with a goal.

use std::path::{Path, PathBuf};

use moor_core::config::Flags;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewTask};
use moor_core::domain::event::CheckedCred;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::surface::{workspace_label, Worktree};
use moor_core::view::{AgentLaunchView, DotfilesView, SetupView, StartView};
use moor_core::{ErrorKind, StartReq, TaskId, Warning};

use super::common::activity;
use super::common::autosleep;
use super::common::boxlock::{self, BoxLock};
use super::common::capacity;
use super::common::facts::{egress_plan, egress_support, settings};
use super::common::provision::{provision, reprovision, reread, Entry, Provisioned};
use super::common::provisioning::dead_provisioning;
use super::common::view::task_view;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, StoreError, TaskRow};

mod base;
mod launch;
mod preflight;
mod relaunch;

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

/// Where the Task works.
struct Placed {
    /// Worktree and its branch.
    wt: Worktree,
    /// Workspace id.
    workspace: String,
}

/// Starts or attaches to the Task named in `req`.
///
/// # Errors
/// [`CommandError`] when a read, a seam call or a store write fails.
pub(crate) fn run(cx: &Cx, req: &StartReq) -> Result<StartView, CommandError> {
    let store = cx.store()?;
    let (repo, bound) = locate(cx, req)?;
    settle_all(cx, store)?;
    if let Some(row) = store.open_task_by_name(&req.name)? {
        return attach(cx, store, req, &row);
    }
    if req.resume {
        return Err(relaunch::nothing_to_resume());
    }
    let facts = gather(cx, req, repo, bound)?;
    let flags = Flags {
        model: req.model.clone(),
    };
    let cfg = settings(cx, facts.bound.as_deref().unwrap_or(&facts.repo), &flags)?;
    egress_plan(&cfg, egress_support(cx, &cfg)?)?;
    if let Some(reason) =
        preflight::rejection(cx, &cfg, facts.bound.as_deref().unwrap_or(&facts.repo))
    {
        return reject(cx, store, req, &facts, &reason);
    }
    let placed = match place(cx, req, &facts) {
        Ok(placed) => placed,
        Err(e) => return raced(cx, store, req, e),
    };
    let id = match insert(store, req, &facts, &placed, preflight::checked(&cfg)) {
        Ok(id) => id,
        Err(e) => return raced(cx, store, req, e),
    };
    let (done, agent) = {
        let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
        let done = if reread(store, id)?.boxed.is_none() {
            provision(cx, store, id, Entry::Fresh, &cfg)?
        } else {
            Provisioned::skipped()
        };
        let agent = launch::run(cx, store, req, id, done.setup.exit(), &|| {
            Ok(cfg.model.as_ref().map(|m| m.value.clone()))
        })?;
        (done, agent)
    };
    launch::watch(cx, store, id, &agent);
    let row = reread(store, id)?;
    let mut v = queued_view(cx, &row, false, &cfg.provider.value);
    v.setup = done.setup.view();
    v.dotfiles = done.dotfiles.view();
    set_agent(&mut v, agent);
    Ok(v)
}

/// Records a rejected Task, with no worktree or Workspace, and fails with the outcome.
fn reject(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
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
    let new = NewTask {
        name: req.name.clone(),
        repo: facts.repo.display().to_string(),
    };
    let id = store.apply(Decision::insert(new).set(set))?.task;
    let rejected = TaskInput::Rejected {
        reason: reason.to_owned(),
    };
    store.apply(Decision::new(id).task_input(rejected))?;
    let row = reread(store, id)?;
    let shown = serde_json::to_value(view(cx, &row, false))
        .map_err(|e| CommandError::new(ErrorKind::Internal, e))?;
    Err(CommandError::new(ErrorKind::Failed, reason.to_owned()).with_value(shown))
}

/// Another start made the Task first: attach to it, else report `lost`.
fn raced(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    lost: impl Into<CommandError>,
) -> Result<StartView, CommandError> {
    let open = store.open_task_by_name(&req.name)?;
    open.map_or_else(|| Err(lost.into()), |row| attach(cx, store, req, &row))
}

/// Starts over on an open Task whose Box is lost; otherwise only attaches.
fn attach(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    row: &TaskRow,
) -> Result<StartView, CommandError> {
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
    let reprovisioned = reprovision(cx, store, row, "start")?;
    store.apply(Decision::new(row.id).event(ExtraEvent::Attached))?;
    let setup_exit = reprovisioned.as_ref().and_then(|p| p.setup.exit());
    let model = || {
        let at = row.worktree.as_deref().unwrap_or(&row.repo);
        let flags = Flags {
            model: req.model.clone(),
        };
        let cfg = settings(cx, Path::new(at), &flags)?;
        Ok(cfg.model.map(|m| m.value))
    };
    wake_idle(store, row.id)?;
    let agent = relaunch::run(cx, store, req, row.id, setup_exit, &model)?;
    launch::watch(cx, store, row.id, &agent);
    let row = &reread(store, row.id)?;
    let mut v = with_provider(cx, row, true)?;
    if let Some(p) = reprovisioned {
        v.setup = p.setup.view();
        v.dotfiles = p.dotfiles.view();
    }
    set_agent(&mut v, agent);
    Ok(v)
}

/// Moves an Idle Task being started to Working, so the launch check sees a Working Task.
fn wake_idle(store: &Store, id: TaskId) -> Result<(), CommandError> {
    let row = reread(store, id)?;
    if row.state != TaskState::Idle {
        return Ok(());
    }
    match store.apply(Decision::new(id).task_input(TaskInput::AgentBusy)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Puts the launch outcome in the view, naming a failure in a warning too.
fn set_agent(v: &mut StartView, agent: AgentLaunchView) {
    if let Some(f) = &agent.reason {
        v.warnings_inline.push(f.text());
    }
    v.agent = agent;
}

/// Moves each open Task with a ready Box and no failed setup between Working and Idle.
///
/// Runs before the start, which may then leave the Task it starts in another state.
fn settle_all(cx: &Cx, store: &Store) -> Result<(), CommandError> {
    for row in &store.open_tasks()? {
        let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
        if !ready || row.setup_failed_at.is_some() {
            continue;
        }
        let settled = activity::read(cx, store, row, cx.clock().now())
            .and_then(|reading| activity::settle(cx, store, row, &reading));
        if let Err(e) = settled {
            cx.warn(Warning {
                code: "settle_failed".to_owned(),
                message: format!("could not settle task {}: {}", row.id.get(), e.error),
            });
            continue;
        }
        autosleep::run(cx, store, row.id);
    }
    Ok(())
}

/// [`queued_view`] for a row whose provider is not at hand; resolved only when it is queued.
fn with_provider(cx: &Cx, row: &TaskRow, attached: bool) -> Result<StartView, CommandError> {
    if row.state != TaskState::WaitingForBox {
        return Ok(view(cx, row, attached));
    }
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let cfg = settings(cx, Path::new(at), &Flags::default())?;
    Ok(queued_view(cx, row, attached, &cfg.provider.value))
}

/// The response for `row`, with the limit that queued it named in a warning.
fn queued_view(cx: &Cx, row: &TaskRow, attached: bool, provider: &str) -> StartView {
    let mut v = view(cx, row, attached);
    if v.queued {
        if let Some(lim) = capacity::limit(cx.config(), provider) {
            v.warnings_inline.push(format!(
                "waiting for a free box: {} allows {} at a time",
                lim.key, lim.max
            ));
        }
    }
    v
}

/// The response for `row`; nothing past the Box runs yet.
fn view(cx: &Cx, row: &TaskRow, attached: bool) -> StartView {
    StartView {
        task: task_view(row, cx.config()),
        attached,
        queued: row.state == TaskState::WaitingForBox,
        promoted: Vec::new(),
        setup: SetupView::default(),
        dotfiles: DotfilesView::default(),
        agent: AgentLaunchView::default(),
        warnings_inline: Vec::new(),
    }
}

/// An absolute form of `path`, relative to the invocation directory.
fn resolve(cx: &Cx, path: &str) -> PathBuf {
    cx.cwd().join(path)
}

/// The repo root and the bound worktree; refuses bad input before any change.
fn locate(cx: &Cx, req: &StartReq) -> Result<(PathBuf, Option<PathBuf>), CommandError> {
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
    Ok((repo, bound))
}

/// Reads the base the Task starts from; changes nothing.
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
/// Doing it first means a failure leaves no worktree without a Task row.
fn place(cx: &Cx, req: &StartReq, facts: &Facts) -> Result<Placed, CommandError> {
    cx.git()
        .exclude_scratch(facts.bound.as_deref().unwrap_or(&facts.repo))
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    let wt = match &facts.bound {
        Some(path) => Worktree {
            path: path.clone(),
            branch: String::new(),
        },
        None => cx
            .surface()
            .create_worktree(&facts.repo, &req.name, facts.from.as_deref())?,
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

/// Inserts the Task row with everything known before the Box.
fn insert(
    store: &Store,
    req: &StartReq,
    facts: &Facts,
    placed: &Placed,
    checked: Vec<CheckedCred>,
) -> Result<TaskId, StoreError> {
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
    let new = NewTask {
        name: req.name.clone(),
        repo: facts.repo.display().to_string(),
    };
    let mut decision = Decision::insert(new).set(set);
    if !checked.is_empty() {
        decision = decision.event(ExtraEvent::Checked {
            credentials: checked,
        });
    }
    Ok(store.apply(decision)?.task)
}

#[cfg(test)]
mod tests;
