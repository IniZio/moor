//! Arguments of the `webhook-*` requests.

use serde::{Deserialize, Serialize};

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
