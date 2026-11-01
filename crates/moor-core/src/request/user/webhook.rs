//! Arguments of the `webhook-*` requests.

use serde::{Deserialize, Serialize};

use super::SecretValue;
use crate::user::Identity;

/// Arguments of `webhook-add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookAddReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The `https://` URL each event is posted to.
    pub url: String,
    /// The event kinds to post; the defaults when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<String>>,
    /// The header the receiver wants its own token in: `Authorization` or an `X-` name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
}

/// Arguments of `webhook-remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookRemoveReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The webhook's id.
    pub id: i64,
}

/// Arguments of `webhook-setup-show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSetupShowReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The webhook's id.
    pub id: i64,
}

/// Arguments of `webhook-setup-finish`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookSetupFinishReq {
    /// The identity asserted; the controller socket's own peer when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The webhook's id.
    pub id: i64,
    /// The value of the webhook's header; required exactly when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_value: Option<SecretValue>,
}
