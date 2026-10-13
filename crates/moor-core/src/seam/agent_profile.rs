//! Agent profile seam: turns an agent's hook state file and pane text into neutral readings.

use crate::activity::HookRecord;
use crate::name::{BoxPath, NameError};

/// What a launch needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCtx {
    /// The model to ask for; the agent's default when `None`.
    pub model: Option<String>,
    /// The Box workdir the agent runs in.
    pub workdir: BoxPath,
    /// Whether the agent may skip its permission prompts.
    pub bypass: bool,
    /// The Box user's home directory.
    pub home: BoxPath,
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

/// What is specific to one kind of agent.
pub trait AgentProfile: Send + Sync {
    /// Parses the agent's hook state file; `None` when it is unreadable or partial.
    fn parse_hook(&self, raw: &[u8]) -> Option<HookRecord>;

    /// The pending question in the pane text, apart from the agent's input box and footer.
    fn pane_prompt(&self, tail: &str) -> Option<String>;

    /// The first-run prompt the pane text shows, when one holds the agent.
    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch>;

    /// Arguments for a fresh session that reads its brief.
    fn argv(&self, ctx: &LaunchCtx) -> Vec<String>;

    /// Arguments that continue the stored session `session_id`.
    fn resume_argv(&self, session_id: &str, ctx: &LaunchCtx) -> Vec<String>;

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
}
