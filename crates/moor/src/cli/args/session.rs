//! Arguments of the `session` verbs.

use clap::{ArgGroup, Args, Subcommand};
use moor_core::request::OpenSessionReq;
use moor_core::BoxName;

use super::task_name;

/// Verbs under `session`.
#[derive(Debug, Subcommand)]
pub(super) enum SessionCmd {
    /// Start a new agent Session on an existing Box.
    Open(OpenArgs),
}

/// Arguments of `session open`.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("seed_src").required(true).args(["seed", "seed_file"])))]
pub(super) struct OpenArgs {
    /// Task name.
    #[arg(value_parser = task_name)]
    task: BoxName,
    /// Text for the agent's first prompt.
    #[arg(long, value_name = "TEXT")]
    seed: Option<String>,
    /// Read the first prompt from this file, used as is.
    #[arg(long, value_name = "PATH", value_parser = read_seed)]
    seed_file: Option<String>,
}

/// clap parser for `--seed-file`: the file's content, verbatim.
fn read_seed(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

impl From<SessionCmd> for OpenSessionReq {
    fn from(cmd: SessionCmd) -> Self {
        let SessionCmd::Open(a) = cmd;
        Self {
            task: a.task.as_str().to_string(),
            seed: a.seed.or(a.seed_file).unwrap_or_default(),
        }
    }
}
