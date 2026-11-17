//! Arguments of `step-report`, which the gate sends for the agent in a Box.

use serde::{Deserialize, Serialize};

use crate::name::BoxId;
use crate::workflow::StepReport;

/// Arguments of `step-report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepReportReq {
    /// The Box whose agent reported. The gate takes it from `X-Moor-Box`, which only the Proxy
    /// sets, and never from the agent's arguments.
    pub box_id: BoxId,
    /// What the agent reported.
    pub report: StepReport,
}
