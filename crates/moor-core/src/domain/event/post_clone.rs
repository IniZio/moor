//! The steps that run in a Box between the clone and the agent's launch.

use serde::Serialize;

mod provider_step;
pub use provider_step::{ProviderStep, StepName, MAX_STEPS};

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
    /// The driver acquiring the machine.
    Acquire,
    /// Starting the Box's Proxy.
    Proxy,
    /// Cloning the other listed repos, all at once.
    CloneOthers,
    /// The engine launching the agent.
    Launch,
    /// The profile writers in the agent's start script, which are mostly marketplace adds and plugin installs.
    Plugins,
}

/// How long a part of setting the Box up took, or which step after the clone is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
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
    /// The Box started on a fork of the deepest setup checkpoint an earlier Box saved.
    WarmStarted {
        /// Setup lines the checkpoint already holds; 0 is the clone alone.
        layer: u32,
        /// How many lines the repo declares.
        layers: u32,
        /// What the clone and those lines took the Box that saved them, less the fork.
        saved_ms: u64,
        /// Time the fork took, in milliseconds.
        fork_ms: u64,
        /// What the provider reports the fork spent that time on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        steps: Vec<ProviderStep>,
    },
    /// The fork of a setup checkpoint failed, so the Box is set up from scratch.
    WarmStartFailed {
        /// Setup lines the checkpoint held.
        layer: u32,
        /// What the provider said.
        error: String,
    },
    /// The Box started without cache volumes although its plan would have kept some.
    CacheCold {
        /// Why, as a snake-case name such as `switched_off` or `machine_offline`.
        reason: String,
        /// The reason in a sentence.
        detail: String,
    },
    /// A restored cache volume failed a restore step, so the Box emptied it and went on.
    CacheEmptied {
        /// The volume.
        volume: String,
        /// Why, as the step printed it.
        reason: String,
    },
    /// A setup layer's checkpoint was taken while the Box was paused, or the attempt failed.
    CheckpointTaken {
        /// Setup lines the checkpoint holds; 0 is the clone alone.
        layer: u32,
        /// The layer's key.
        key: String,
        /// The provider's checkpoint id; none when the attempt failed.
        checkpoint: Option<String>,
        /// Time the Box was paused for it, in milliseconds.
        duration_ms: u64,
        /// What the provider said when the attempt failed.
        error: Option<String>,
    },
    /// A setup checkpoint was deleted to keep the provider's disk above its floor.
    CheckpointEvicted {
        /// The provider's checkpoint id.
        checkpoint: String,
        /// The repo whose setup it held, `OWNER/NAME`.
        repo: String,
        /// Its size on the host, when known.
        bytes: Option<u64>,
        /// Why it went before others: `superseded` or `least_recently_used`.
        reason: String,
        /// Free disk when it was picked, in MiB.
        free_mib: u64,
        /// The free disk the floor asks for, in MiB.
        needs_mib: u64,
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
            Self::WarmStarted { .. } => "warm_started",
            Self::WarmStartFailed { .. } => "warm_start_failed",
            Self::CacheCold { .. } => "cache_cold",
            Self::CacheEmptied { .. } => "cache_emptied",
            Self::CheckpointTaken { .. } => "checkpoint_taken",
            Self::CheckpointEvicted { .. } => "checkpoint_evicted",
        }
    }
}
