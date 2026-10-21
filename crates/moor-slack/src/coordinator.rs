//! The coordinator agent the front runs for a thread, one process per turn. [`Agent`] is what
//! differs between agents; only [`claude`] knows Claude Code.

use std::path::Path;
use std::process::Command;

use crate::credentials::{AgentKind, CredentialCheck};

pub(crate) mod claude;
mod process;
pub(crate) mod turn;

/// What one turn's command needs from the front.
pub(crate) struct TurnSetup<'a> {
    /// The model to run.
    pub(crate) model: &'a str,
    /// The agent's own config dir for the person who pays for the turn.
    pub(crate) config_dir: &'a Path,
    /// The thread's workdir, where the process runs.
    pub(crate) workdir: &'a Path,
    /// The standing instructions of every turn.
    pub(crate) brief: &'a str,
}

/// What the agent loaded, as its first output line reports it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Loaded {
    /// Tools the model can call.
    pub(crate) tools: Vec<String>,
    /// Tool servers it connected to.
    pub(crate) servers: Vec<String>,
    /// Skills it loaded.
    pub(crate) skills: Vec<String>,
    /// Slash commands it loaded.
    pub(crate) commands: Vec<String>,
    /// Plugins it loaded; logged, never checked, because no flag removes the built-in ones.
    pub(crate) plugins: Vec<String>,
}

/// How the agent said a turn ended.
#[derive(Debug, PartialEq)]
pub(crate) enum Answer {
    /// The reply to post, and the cost the agent reported.
    Reply {
        /// The reply text.
        text: String,
        /// The cost in US dollars, when reported.
        cost_usd: Option<f64>,
    },
    /// The agent's provider refused the credential.
    Refused,
    /// The turn ended in an error; the text is the error's first line.
    Failed(String),
    /// The agent could not run the turn; the text says why.
    Unavailable(String),
}

/// One line of the agent's output, as the front reads it.
#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    /// The report of what the agent loaded.
    Loaded(Loaded),
    /// The end of the turn.
    Answer(Answer),
    /// Anything else.
    Other,
}

/// What is specific to one kind of coordinator agent: the command of a turn with its cleared
/// environment, where the credential goes, how the output reads, and what makes a workdir
/// unsafe for it.
pub(crate) trait Agent: CredentialCheck + Send + Sync {
    /// Which agent this is.
    fn kind(&self) -> AgentKind;

    /// The program's name, such as `Claude Code`.
    fn program(&self) -> &'static str;

    /// The version the binary reported.
    fn version(&self) -> &str;

    /// The command of one turn, with its environment cleared and set. Input goes on stdin.
    fn turn_command(&self, setup: &TurnSetup<'_>) -> Command;

    /// The descriptor the turn reads its credential on, from a pipe.
    fn credential_fd(&self) -> i32;

    /// What one line of the turn's output says.
    fn event(&self, line: &str) -> Event;

    /// Why the agent must not run in `workdir`, such as an instruction file that it would load
    /// from the directory or one of its ancestors. An error reading the tree is a reason too.
    fn unsafe_workdir(&self, workdir: &Path) -> Option<String>;
}
