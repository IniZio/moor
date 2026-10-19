//! Arguments of the `session` verbs.

use clap::{ArgGroup, Args, Subcommand};
use moor_core::request::{OpenHostSessionReq, OpenSessionReq, Request};
use moor_core::BoxName;

use super::task_name;

/// Verbs under `session`.
#[derive(Debug, Subcommand)]
pub(super) enum SessionCmd {
    /// Start a new agent Session on an existing Box.
    Open(OpenArgs),
}

/// Arguments of `session open`: a Box Session (task + seed) or, with `--host`, a host Session.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("seed_src").args(["seed", "seed_file"])))]
pub(super) struct OpenArgs {
    /// Task name.
    #[arg(value_parser = task_name, required_unless_present = "host", conflicts_with = "host")]
    task: Option<BoxName>,
    /// Text for the agent's first prompt.
    #[arg(long, value_name = "TEXT", required_unless_present_any = ["seed_file", "host"], conflicts_with = "host")]
    seed: Option<String>,
    /// Read the first prompt from this file, used as is.
    #[arg(long, value_name = "PATH", value_parser = read_seed, conflicts_with = "host")]
    seed_file: Option<String>,
    /// Open a Session on the host, without a Box.
    #[arg(long, requires = "workdir")]
    host: bool,
    /// Directory the host Session works in; a relative path is taken from the current dir.
    #[arg(long, value_name = "DIR", value_parser = absolute_dir, requires = "host")]
    workdir: Option<String>,
    /// Agent to run in the host Session.
    #[arg(long, value_name = "NAME", requires = "host")]
    agent: Option<String>,
    /// Existing agent session to resume in the host Session.
    #[arg(long, value_name = "ID", requires = "host")]
    agent_session_id: Option<String>,
}

/// clap parser for `--seed-file`: the file's content, verbatim.
fn read_seed(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// clap parser for `--workdir`: the path made absolute against the current dir.
fn absolute_dir(path: &str) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read current dir: {e}"))?;
    Ok(cwd.join(path).to_string_lossy().into_owned())
}

impl From<SessionCmd> for Request {
    fn from(cmd: SessionCmd) -> Self {
        let SessionCmd::Open(a) = cmd;
        match a.workdir {
            Some(workdir) => Self::OpenHostSession(OpenHostSessionReq {
                workdir,
                agent: a.agent,
                agent_session_id: a.agent_session_id,
            }),
            None => Self::OpenSession(OpenSessionReq {
                task: a.task.map(|t| t.as_str().to_string()).unwrap_or_default(),
                seed: a.seed.or(a.seed_file).unwrap_or_default(),
            }),
        }
    }
}
