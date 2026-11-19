//! The identity a request's own field asserts.

use super::{
    AsReq, EnrollReq, InboxReq, McpAddReq, McpOAuthFinishReq, McpRemoveReq, ProfileCheckReq,
    ProfileSetReq, SecretDeleteReq, SecretSetReq, StartNeedsReq, WebhookAddReq, WebhookRemoveReq,
    WebhookSetupFinishReq, WebhookSetupShowReq,
};
use crate::request::{
    MachineEnrollReq, MachineRemoveReq, MachineStatusReq, NodeSettingsGetReq, NodeSettingsSetReq,
    RepoConfigGetReq, RepoConfigSetReq, Request, StartReq,
};
use crate::user::Identity;

impl Request {
    /// The identity this request's own field asserts.
    pub(super) const fn own_identity(&self) -> Option<&Identity> {
        match self {
            Self::Start(StartReq { identity, .. })
            | Self::StartNeeds(StartNeedsReq { identity, .. })
            | Self::RepoConfigGet(RepoConfigGetReq { identity, .. })
            | Self::RepoConfigSet(RepoConfigSetReq { identity, .. })
            | Self::Whoami(AsReq { identity })
            | Self::Enroll(EnrollReq { identity, .. })
            | Self::SecretNames(AsReq { identity })
            | Self::McpList(AsReq { identity })
            | Self::ProfileShow(AsReq { identity })
            | Self::ProfileRemove(AsReq { identity })
            | Self::ProfileCheck(ProfileCheckReq { identity, .. })
            | Self::WebhookList(AsReq { identity })
            | Self::Inbox(InboxReq { identity, .. })
            | Self::WebhookAdd(WebhookAddReq { identity, .. })
            | Self::WebhookRemove(WebhookRemoveReq { identity, .. })
            | Self::WebhookSetupShow(WebhookSetupShowReq { identity, .. })
            | Self::WebhookSetupFinish(WebhookSetupFinishReq { identity, .. })
            | Self::ProfileSet(ProfileSetReq { identity, .. })
            | Self::MachineList(AsReq { identity })
            | Self::NodeList(AsReq { identity })
            | Self::NodeSettingsGet(NodeSettingsGetReq { identity, .. })
            | Self::NodeSettingsSet(NodeSettingsSetReq { identity, .. })
            | Self::MachineRemove(MachineRemoveReq { identity, .. })
            | Self::MachineStatus(MachineStatusReq { identity, .. })
            | Self::MachineEnroll(MachineEnrollReq { identity, .. })
            | Self::McpAdd(McpAddReq { identity, .. })
            | Self::McpRemove(McpRemoveReq { identity, .. })
            | Self::McpOAuthFinish(McpOAuthFinishReq { identity, .. })
            | Self::SecretSet(SecretSetReq { identity, .. })
            | Self::SecretDelete(SecretDeleteReq { identity, .. }) => identity.as_ref(),
            other => super::super::workflow::own_identity(other),
        }
    }
}
