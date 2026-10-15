//! Arguments of the Task-scoped verbs that take only a name plus a flag or path.

use clap::Args;
use moor_core::{CloseReq, CpReq, RemoveReq, TaskName, TaskReq};

use super::task_name;

/// Arguments of `close`.
#[derive(Debug, Args)]
pub(super) struct CloseArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    pub(super) task: TaskName,
    /// Close despite unsaved work.
    #[arg(long)]
    pub(super) force: bool,
    /// Run teardown even when the Task's .moor.yaml differs from its base commit.
    #[arg(long)]
    pub(super) trust_moor_yaml: bool,
}

impl From<CloseArgs> for CloseReq {
    fn from(a: CloseArgs) -> Self {
        Self {
            task: a.task,
            force: a.force,
            trust_moor_yaml: a.trust_moor_yaml,
        }
    }
}

/// Arguments of `remove`.
#[derive(Debug, Args)]
pub(super) struct RemoveArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    pub(super) task: TaskName,
    /// Discard the Task's work.
    #[arg(long)]
    pub(super) discard: bool,
}

impl From<RemoveArgs> for RemoveReq {
    fn from(a: RemoveArgs) -> Self {
        Self {
            task: a.task,
            discard: a.discard,
        }
    }
}

/// Arguments of `cp`.
#[derive(Debug, Args)]
pub(super) struct CpArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    pub(super) task: TaskName,
    /// Source path in the Box; relative to its workdir and may not contain
    /// `..`. Use an absolute Box path to reach outside the workdir.
    pub(super) src: String,
    /// Destination path on the host; relative to the current directory.
    pub(super) dest: String,
}

impl From<CpArgs> for CpReq {
    fn from(a: CpArgs) -> Self {
        Self {
            task: a.task,
            src: a.src,
            dest: a.dest,
        }
    }
}

/// A lone Task argument.
#[derive(Debug, Args)]
pub(super) struct TaskArg {
    /// Task name.
    #[arg(value_parser = task_name)]
    pub(super) task: TaskName,
}

impl From<TaskArg> for TaskReq {
    fn from(a: TaskArg) -> Self {
        Self { task: a.task }
    }
}
