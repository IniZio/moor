//! Arguments of the Box-scoped verbs that take only a name plus a flag or path.

use clap::Args;
use moor_core::{BoxName, BoxReq, CloseReq, CpReq};

use super::box_name;

/// Arguments of `close`.
#[derive(Debug, Args)]
pub(super) struct CloseArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    pub(super) name: BoxName,
    /// Close despite unsaved work.
    #[arg(long)]
    pub(super) force: bool,
}

impl From<CloseArgs> for CloseReq {
    fn from(a: CloseArgs) -> Self {
        Self {
            target: a.name.into(),
            force: a.force,
        }
    }
}

/// Arguments of `cp`.
#[derive(Debug, Args)]
pub(super) struct CpArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    pub(super) name: BoxName,
    /// Source path in the Box; relative to its workdir and may not contain
    /// `..`. Use an absolute Box path to reach outside the workdir.
    pub(super) src: String,
    /// Destination path on the host; relative to the current directory.
    pub(super) dest: String,
}

impl From<CpArgs> for CpReq {
    fn from(a: CpArgs) -> Self {
        Self {
            box_name: a.name,
            src: a.src,
            dest: a.dest,
        }
    }
}

/// A lone Box argument.
#[derive(Debug, Args)]
pub(super) struct BoxArg {
    /// Box name.
    #[arg(value_parser = box_name)]
    pub(super) name: BoxName,
}

impl From<BoxArg> for BoxReq {
    fn from(a: BoxArg) -> Self {
        Self { box_name: a.name }
    }
}
