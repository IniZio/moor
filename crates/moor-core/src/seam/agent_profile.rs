//! Agent profile seam: turns an agent's hook state file and pane text into neutral readings.

use std::fmt;

use crate::activity::HookRecord;
use crate::name::{BoxPath, NameError};
use crate::seam::engine::LaunchPrompt;
use crate::secret::Secret;

/// The Proxy's address as the Box sees it.
pub struct ProxyAddr {
    /// The Proxy tunnel's port as seen inside the Box.
    pub box_port: u16,
    /// The loopback port of the egress proxy, which sends only `hosts` through the tunnel.
    pub egress_port: u16,
    /// The public Box CA certificate in PEM form; not secret.
    pub ca_pem: String,
    /// Non-secret environment pairs from the tool rules, already resolved.
    pub box_env: Vec<(String, String)>,
    /// Whether a forge rule routes GitHub through the Proxy, so the agent gets `gh`.
    pub forge: bool,
    /// The hosts the credential rules serve; every other host is dialed direct.
    pub hosts: Vec<String>,
}

impl fmt::Debug for ProxyAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyAddr")
            .field("box_port", &self.box_port)
            .field("box_env", &self.box_env)
            .field("forge", &self.forge)
            .field("egress_port", &self.egress_port)
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

/// An environment variable for the agent process whose value is secret.
///
/// It must be staged into the agent's environment and never put on the command line.
pub struct EnvVar {
    /// The variable name.
    pub name: String,
    /// The secret value.
    pub value: Secret,
}

impl fmt::Debug for EnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvVar")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// What a launch needs to know.
#[derive(Debug)]
pub struct LaunchCtx {
    /// The model to ask for; the agent's default when `None`.
    pub model: Option<String>,
    /// The Box workdir the agent runs in.
    pub workdir: BoxPath,
    /// The Proxy the agent reaches its model through; the agent may skip its
    /// permission prompts only when this is set.
    pub proxy: Option<ProxyAddr>,
    /// The Box user's home directory.
    pub home: BoxPath,
    /// The first prompt a fresh session starts with.
    pub prompt: LaunchPrompt,
}

/// A file to put in the Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxFile {
    /// Where it goes.
    pub path: BoxPath,
    /// What it holds.
    pub content: String,
    /// Unix permission bits.
    pub mode: u32,
}

/// Files to write and a POSIX sh step to run after them, on the Box.
///
/// The step exits non-zero when it fails; a step that only warns exits zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSetup {
    /// Written before the step runs.
    pub files: Vec<BoxFile>,
    /// Shell text, safe to run any number of times.
    pub step: String,
}

/// A key to send to the agent's pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character.
    Char(char),
}

/// A first-run prompt that holds the agent until someone answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstRunPrompt {
    /// Asks to confirm running without permission checks.
    BypassPermissions,
    /// Asks whether to trust the working folder.
    FolderTrust,
    /// Waits for Enter to go on.
    PressEnter,
    /// Shows the agent's security notes.
    SecurityNotes,
    /// Asks for a colour theme.
    Theme,
}

impl FirstRunPrompt {
    /// The `snake_case` name recorded in events and warnings.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BypassPermissions => "bypass_permissions",
            Self::FolderTrust => "folder_trust",
            Self::PressEnter => "press_enter",
            Self::SecurityNotes => "security_notes",
            Self::Theme => "theme",
        }
    }
}

/// A first-run prompt found in pane text, with the line that showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMatch {
    /// Which prompt.
    pub prompt: FirstRunPrompt,
    /// The matched pane line, without ANSI codes or box-drawing edges.
    pub line: String,
}

/// How to put an agent on a Box that lacks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInstall {
    /// POSIX sh text run on the Box as its user. It exits 0 at once when the agent command is
    /// present, installs it otherwise, and exits non-zero when the command is still missing. It
    /// is safe to run any number of times.
    pub script: String,
    /// The directories, in `~/` form, that hold what the script installs.
    pub dirs: Vec<String>,
}

/// What a transcript script prints when the agent left no transcript.
pub const NOTHING_TO_ARCHIVE: &str = "moor: nothing to archive";

/// What is specific to one kind of agent.
pub trait AgentProfile: Send + Sync {
    /// Parses the agent's hook state file; `None` when it is unreadable or partial.
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord>;

    /// The pending question in the pane text, apart from the agent's input box and footer.
    fn pane_prompt(&self, tail: &str) -> Option<String>;

    /// The first-run prompt the pane text shows, when one holds the agent.
    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch>;

    /// Whether the agent runs no hook while a first-run screen holds its pane, so a hook report
    /// after the launch's starting marker shows it is past those screens.
    ///
    /// An agent that may report while one is up answers `false`, and the first-run prompt watch
    /// then runs its whole window.
    fn hooks_quiet_during_first_run(&self) -> bool {
        false
    }

    /// How to install the agent on a Box that lacks it; `None` when this agent has no installer
    /// moor knows, and the Box must bring its own.
    fn install(&self) -> Option<AgentInstall> {
        None
    }

    /// Arguments for a fresh session that reads its brief.
    fn argv(&self, ctx: &LaunchCtx) -> Vec<String>;

    /// Arguments that continue the stored session `session_id`.
    fn resume_argv(&self, session_id: &str, ctx: &LaunchCtx) -> Vec<String>;

    /// Environment variables the agent needs; empty without a Proxy.
    ///
    /// Values never go in argv.
    fn env(&self, ctx: &LaunchCtx) -> Vec<EnvVar>;

    /// Records the agent's first-run answers so it opens without prompts.
    ///
    /// The step only warns on failure.
    ///
    /// # Errors
    /// Returns a [`NameError`] when a path cannot be formed.
    fn preseed(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError>;

    /// Installs the hooks that write the state file under the workdir.
    ///
    /// Existing agent settings stay; the step fails when it cannot merge.
    ///
    /// # Errors
    /// Returns a [`NameError`] when a path cannot be formed.
    fn hook_writer(&self, ctx: &LaunchCtx) -> Result<BoxSetup, NameError>;

    /// The pane text, in lower case, that shows the agent is working on a prompt.
    fn busy_marker(&self) -> &str;

    /// The keys that pick menu option `n`.
    fn menu_keys(&self, n: u8) -> Vec<Key>;

    /// POSIX sh text that archives the agent's session transcripts to `archive`.
    ///
    /// It runs in the Box workdir. When there is nothing to archive it prints
    /// [`NOTHING_TO_ARCHIVE`] and exits 0; any other non-zero exit is a failure.
    fn transcript_script(&self, archive: &str) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"sentinel-9c2e";

    #[test]
    fn env_var_debug_hides_value() {
        let var = EnvVar {
            name: "TOKEN".to_owned(),
            value: Secret::new(SECRET.to_vec()),
        };
        let text = format!("{var:?}");
        assert!(text.contains("TOKEN"));
        assert!(!text.contains("sentinel-9c2e"));
    }
}
