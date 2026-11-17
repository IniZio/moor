//! Arguments of the Box-scoped verbs that take only a name plus a flag or path.

use clap::Args;
use moor_core::config::override_text;
use moor_core::request::{BoxConfigSetReq, CloseFailedReq};
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

/// Arguments of `close-failed`.
#[derive(Debug, Args)]
pub(super) struct CloseFailedArgs {
    /// Close despite unsaved work.
    #[arg(long)]
    pub(super) force: bool,
}

impl From<CloseFailedArgs> for CloseFailedReq {
    fn from(a: CloseFailedArgs) -> Self {
        Self { force: a.force }
    }
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

/// Arguments of `box-config`.
#[derive(Debug, Args)]
pub(super) struct BoxConfigArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    pub(super) name: BoxName,
    /// The Box's whole override, as a partial `.moor.yaml`; `-` reads stdin.
    #[arg(
        long = "file",
        short = 'f',
        value_name = "FILE",
        default_value = "-",
        value_parser = super::config_text,
        conflicts_with = "remove"
    )]
    pub(super) text: String,
    /// Remove the Box's override, so it reads its repo's config alone.
    #[arg(long)]
    pub(super) remove: bool,
}

impl From<BoxConfigArgs> for BoxConfigSetReq {
    fn from(a: BoxConfigArgs) -> Self {
        Self {
            box_name: a.name,
            config: if a.remove {
                None
            } else {
                override_text(Some(a.text))
            },
        }
    }
}
