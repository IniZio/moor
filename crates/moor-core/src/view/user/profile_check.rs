//! Value of `profile-check`: what a profile resolves to and what failed.

use serde::{Deserialize, Serialize};

use crate::domain::event::ProfileStep;
use crate::name::ProfileSource;

/// Value of `profile-check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileCheckValue {
    /// What was checked; none when no profile is set.
    pub source: Option<ProfileSource>,
    /// The profile at its commit; none when the ref did not resolve.
    pub resolved: Option<ProfileResolved>,
    /// Each failed step with why; empty when the profile is valid.
    pub failures: Vec<ProfileProblem>,
    /// The newest `profile_failed` event on any of the user's Boxes.
    pub last_failed: Option<ProfileLastFailed>,
}

/// A profile at the commit its ref resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileResolved {
    /// The full commit sha.
    pub sha: String,
    /// Whether `setup.sh` exists at that commit.
    pub setup: bool,
    /// What the manifest installs for Claude Code.
    pub claude: AgentInstalls,
    /// What the manifest installs for Codex.
    pub codex: AgentInstalls,
    /// Why a repo the profile installs from did not resolve, one per repo.
    pub unresolved: Vec<String>,
}

/// What a manifest installs for one agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInstalls {
    /// The model it passes to the agent.
    pub model: Option<String>,
    /// The marketplaces it adds.
    pub marketplaces: Vec<NamedRepo>,
    /// The plugins it installs, as `plugin@marketplace` as written.
    pub plugins: Vec<String>,
}

/// A marketplace by its name in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedRepo {
    /// The marketplace's name.
    pub name: String,
    /// Its repo, as the manifest gives it.
    pub repo: String,
}

/// A step that failed, with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileProblem {
    /// The step.
    pub step: ProfileStep,
    /// What went wrong.
    pub detail: String,
}

/// The newest profile failure a Box of the user recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLastFailed {
    /// The step.
    pub step: ProfileStep,
    /// What went wrong.
    pub detail: String,
    /// The Box it happened on.
    pub box_name: String,
    /// When, in Unix milliseconds.
    pub at_ms: i64,
}
