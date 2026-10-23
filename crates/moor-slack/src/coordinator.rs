//! The coordinator agent the front runs for a thread, one process per turn. [`Agent`] is what
//! differs between agents; only [`claude`] knows Claude Code.

use std::path::Path;
use std::process::Command;

use moor_core::secret::Secret;

use crate::credentials::{AgentKind, CredentialCheck};

pub(crate) mod brief;
pub(crate) mod claude;
pub(crate) mod context;
pub(crate) mod mcp;
mod process;
pub(crate) mod tools;
pub(crate) mod turn;

/// The front's tool server for one turn: the program the agent starts as a child for the
/// turn, and the tools it serves.
pub(crate) struct ToolServer<'a> {
    /// The name the agent knows the server by.
    pub(crate) name: &'a str,
    /// Absolute path of the program.
    pub(crate) program: &'a Path,
    /// Its arguments, which name no secret.
    pub(crate) args: &'a [String],
    /// The tools it serves.
    pub(crate) tools: &'a [&'a str],
}

/// What one turn's command needs from the front.
pub(crate) struct TurnSetup<'a> {
    /// The model to run.
    pub(crate) model: &'a str,
    /// The agent's own config dir for the person who pays for the turn.
    pub(crate) config_dir: &'a Path,
    /// The thread's workdir, which holds what the agent is started with. The process runs
    /// elsewhere, in a directory the agent picks.
    pub(crate) workdir: &'a Path,
    /// The standing instructions of every turn.
    pub(crate) brief: &'a str,
    /// The only tool server the agent may load.
    pub(crate) tools: &'a ToolServer<'a>,
    /// How the agent reaches the front Proxy, which holds the credential of who pays.
    pub(crate) proxy: &'a ProxyAccess<'a>,
}

/// How a turn's agent reaches the front Proxy.
pub(crate) struct ProxyAccess<'a> {
    /// The loopback port the front's shim bridges to the Proxy.
    pub(crate) port: u16,
    /// The CA the Proxy signs with.
    pub(crate) ca: &'a Path,
    /// The turn's lease, the proxy password; it stands in for the credential.
    pub(crate) lease: &'a Secret,
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
/// environment and its way to the front Proxy, how the output reads, and what makes a workdir
/// unsafe for it.
pub(crate) trait Agent: CredentialCheck + Send + Sync {
    /// Which agent this is.
    fn kind(&self) -> AgentKind;

    /// The program's name, such as `Claude Code`.
    fn program(&self) -> &'static str;

    /// The version the binary reported.
    fn version(&self) -> &str;

    /// The command of one turn, with its environment cleared and set, wired to the setup's
    /// tool server and to no other. Writes what the agent needs for that into the workdir.
    /// Input goes on stdin.
    fn turn_command(&self, setup: &TurnSetup<'_>) -> Result<Command, String>;

    /// How the agent's load report names `tool` of tool server `server`.
    fn tool_name(&self, server: &str, tool: &str) -> String;

    /// What one line of the turn's output says.
    fn event(&self, line: &str) -> Event;

    /// Why the agent must not run, such as an instruction file that it would load from its
    /// working directory or the machine's managed config. An error reading one is a reason too.
    fn unsafe_cwd(&self) -> Option<String>;
}
