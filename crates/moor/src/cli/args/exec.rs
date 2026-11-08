//! Arguments of `exec`.

use clap::Args;
use moor_core::{BoxName, ExecReq};

use super::box_name;

/// Arguments of `exec`.
#[derive(Debug, Args)]
pub(super) struct ExecArgs {
    /// Box name.
    #[arg(value_parser = box_name)]
    name: BoxName,
    /// Forward stdin even from a terminal; stdin that is not a terminal is always forwarded.
    #[arg(short, long)]
    interactive: bool,
    /// Command and its arguments; a single word is run as a shell line.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    cmd: Vec<String>,
}

impl ExecArgs {
    /// The request, and whether stdin is forwarded from a terminal.
    pub(super) fn into_parts(self) -> (ExecReq, bool) {
        let req = ExecReq {
            target: self.name.into(),
            argv: self.cmd,
            timeout_secs: None,
        };
        (req, self.interactive)
    }
}
