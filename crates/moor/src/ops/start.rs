//! `start` (minimal): create the Task, its worktree and Workspace, acquire the Box.
//!
//! Capacity, setup, dotfiles, credentials and the agent launch land in later slices.

use std::path::{Path, PathBuf};

use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewTask};
use moor_core::seam::surface::{workspace_label, Worktree};
use moor_core::view::{AgentLaunchView, DotfilesView, SetupView, StartView};
use moor_core::{ErrorKind, StartReq, TaskId};

use super::common::view::task_view;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, StoreError, TaskRow};

mod provision;
use provision::{provision, reprovision, Entry};

/// What `start` learns before it changes anything.
struct Facts {
    /// Repo root of the invocation directory.
    repo: PathBuf,
    /// The user's worktree, when `--worktree` names one.
    bound: Option<PathBuf>,
    /// Base ref or the HEAD commit it starts from.
    base: String,
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
    if let Some(row) = store.open_task_by_name(&req.name)? {
        return attach(cx, store, req, &row);
    }
    let facts = gather(cx, req, repo, bound)?;
    let placed = match place(cx, req, &facts) {
        Ok(placed) => placed,
        Err(e) => return raced(cx, store, req, e),
    };
    let id = match insert(store, req, &facts, &placed) {
        Ok(id) => id,
        Err(e) => return raced(cx, store, req, e),
    };
    provision(cx, store, id, Entry::Fresh)?;
    reread(store, id).map(|row| view(&row, false))
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
    check_bound(cx, req, row)?;
    let reprovisioned = reprovision(cx, store, row)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::Attached))?;
    if reprovisioned {
        return reread(store, row.id).map(|r| view(&r, true));
    }
    Ok(view(row, true))
}

/// The stored Task row for `id`.
fn reread(store: &Store, id: TaskId) -> Result<TaskRow, CommandError> {
    store
        .task(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Internal, "started task vanished"))
}

/// The response for `row`; nothing past the Box runs yet.
fn view(row: &TaskRow, attached: bool) -> StartView {
    StartView {
        task: task_view(row),
        attached,
        queued: false,
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

/// Refuses a `--worktree` that differs from the open Task's worktree.
fn check_bound(cx: &Cx, req: &StartReq, row: &TaskRow) -> Result<(), CommandError> {
    let (Some(want), Some(have)) = (&req.worktree, &row.worktree) else {
        return Ok(());
    };
    let want = resolve(cx, want);
    let same = want == Path::new(have)
        || matches!((want.canonicalize(), Path::new(have).canonicalize()),
            (Ok(a), Ok(b)) if a == b);
    if same {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "task {} is already bound to {have}, not {}",
            req.name.as_str(),
            want.display()
        ),
    ))
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
    let base = match &req.base {
        Some(b) => b.clone(),
        None => cx
            .git()
            .head(bound.as_deref().unwrap_or(&repo))
            .map_err(|e| CommandError::new(ErrorKind::Failed, e))?,
    };
    Ok(Facts { repo, bound, base })
}

/// Creates the worktree unless one is bound, then ensures the Workspace.
fn place(cx: &Cx, req: &StartReq, facts: &Facts) -> Result<Placed, CommandError> {
    let wt = match &facts.bound {
        Some(path) => Worktree {
            path: path.clone(),
            branch: String::new(),
        },
        None => cx
            .surface()
            .create_worktree(&facts.repo, &req.name, req.base.as_deref())?,
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
    Ok(store.apply(Decision::insert(new).set(set))?.task)
}

#[cfg(test)]
mod tests;
