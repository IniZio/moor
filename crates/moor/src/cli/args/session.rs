//! Arguments of the `session` verbs.

use clap::{ArgGroup, Args, Subcommand};
use moor_core::request::{
    EndHostSessionReq, OpenHostSessionReq, OpenSessionReq, Request, UpdateHostSessionReq,
};
use moor_core::{BoxName, SessionId};

use super::task_name;

/// Verbs under `session`.
#[derive(Debug, Subcommand)]
pub(super) enum SessionCmd {
    /// Start a new agent Session on an existing Box.
    Open(OpenArgs),
    /// Change the agent or agent session recorded on a host Session.
    Set(SetArgs),
    /// End a host Session.
    End(EndArgs),
}

/// Arguments of `session set`: the Session id and at least one field to change.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("fields").args(["agent", "agent_session_id"]).required(true).multiple(true)))]
pub(super) struct SetArgs {
    /// Numeric Session id.
    id: i64,
    /// Agent to record on the Session.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,
    /// Agent session to record on the Session.
    #[arg(long, value_name = "ID")]
    agent_session_id: Option<String>,
}

/// Arguments of `session end`.
#[derive(Debug, Args)]
pub(super) struct EndArgs {
    /// Numeric Session id.
    id: i64,
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
    /// Parent Session id; defaults to the `MOOR_SESSION` of the calling agent.
    #[arg(long, value_name = "ID", env = "MOOR_SESSION")]
    parent: Option<i64>,
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
        let a = match cmd {
            SessionCmd::Open(a) => a,
            SessionCmd::Set(a) => {
                return Self::UpdateHostSession(UpdateHostSessionReq {
                    session: SessionId::new(a.id),
                    agent: a.agent,
                    agent_session_id: a.agent_session_id,
                })
            }
            SessionCmd::End(a) => {
                return Self::EndHostSession(EndHostSessionReq {
                    session: SessionId::new(a.id),
                })
            }
        };
        match a.workdir {
            Some(workdir) => Self::OpenHostSession(OpenHostSessionReq {
                workdir,
                agent: a.agent,
                agent_session_id: a.agent_session_id,
                parent: a.parent.map(SessionId::new),
            }),
            None => Self::OpenSession(OpenSessionReq {
                task: a.task.map(|t| t.as_str().to_string()).unwrap_or_default(),
                seed: a.seed.or(a.seed_file).unwrap_or_default(),
                parent: a.parent.map(SessionId::new),
            }),
        }
    }
}

#[cfg(test)]
mod tests;
