//! Arguments of `start`.

use serde::{Deserialize, Serialize};

use crate::name::{BoxName, ForgeRepo, SessionId};
use crate::user::Identity;

/// Arguments of `start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "JSON wire shape is a public contract"
)]
pub struct StartReq {
    /// Box name.
    pub name: BoxName,
    /// Prompt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Issue reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// Base revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Agent model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// One of the requester's nodes to run the Box on, in place of their default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Skip starting an agent.
    #[serde(default)]
    pub no_agent: bool,
    /// Resume the agent's stored session instead of starting a fresh one.
    #[serde(default)]
    pub resume: bool,
    /// The repo the Box clones; a Box with no repo starts with an empty workdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Parent Session of the Session this request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// The identity the Box acts for; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}
