//! Arguments of `webhook-claim` and `webhook-report`, which a webhook-delivering front sends.

use serde::{Deserialize, Serialize};

/// Arguments of `webhook-claim`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookClaimReq {
    /// Most deliveries to hand back.
    pub limit: u32,
    /// How long, in milliseconds, the claimed deliveries stay the caller's before another
    /// claim may take them again.
    pub lease_ms: u64,
}

/// Arguments of `webhook-report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookReportReq {
    /// The delivery id `webhook-claim` handed back.
    pub delivery: i64,
    /// How the delivery ended, once the front gave up retrying or succeeded.
    pub outcome: WebhookOutcome,
}

/// How one delivery ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WebhookOutcome {
    /// The receiver accepted the post.
    Delivered,
    /// Every attempt failed.
    Failed {
        /// Why the last attempt failed.
        reason: String,
    },
}
