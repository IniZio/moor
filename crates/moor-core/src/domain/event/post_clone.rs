//! The steps that run in a Box between the clone and the agent's launch.

use serde::Serialize;

/// A step after the clone whose time the Box's events tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostCloneStep {
    /// Unpacking and installing the user's dotfiles.
    Dotfiles,
    /// Installing the agent.
    AgentInstall,
    /// Fetching the agent profile and running its `setup.sh`.
    Profile,
}

/// How long a part of setting the Box up took, or which step after the clone is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimingEvent {
    /// The repo setup script ended.
    SetupFinished {
        /// Time the script took, in milliseconds.
        ms: u64,
        /// Whether every line passed.
        ok: bool,
    },
    /// The repo was cloned in the Box.
    Cloned {
        /// Time the clone, with the git identity set inside it, took in milliseconds.
        ms: u64,
    },
    /// A step after the clone began.
    StepStarted {
        /// Which step.
        step: PostCloneStep,
    },
    /// A step after the clone ended.
    StepFinished {
        /// Which step.
        step: PostCloneStep,
        /// Time the step took, in milliseconds.
        ms: u64,
        /// Whether the step succeeded.
        ok: bool,
    },
    /// One of the repo's `setup:` lines began.
    SetupLineStarted {
        /// Position of the line, counting from 1.
        index: u32,
        /// How many lines the repo declares.
        total: u32,
        /// The line, cut to a length a status line can hold.
        line: String,
    },
    /// One of the repo's `setup:` lines ended.
    SetupLineFinished {
        /// Position of the line, counting from 1.
        index: u32,
        /// How many lines the repo declares.
        total: u32,
        /// Time the line took, in milliseconds.
        ms: u64,
        /// Whether the line exited 0.
        ok: bool,
    },
}

impl TimingEvent {
    /// The `snake_case` kind name; equals the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SetupFinished { .. } => "setup_finished",
            Self::Cloned { .. } => "cloned",
            Self::StepStarted { .. } => "step_started",
            Self::StepFinished { .. } => "step_finished",
            Self::SetupLineStarted { .. } => "setup_line_started",
            Self::SetupLineFinished { .. } => "setup_line_finished",
        }
    }
}
