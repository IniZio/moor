//! Arguments of `machine-pair`, which the gate sends while a machine pairs by code, so every
//! gate replica reads and moves the same code.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Arguments of `machine-pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachinePairReq {
    /// What to do with a code.
    pub op: PairOp,
}

/// One step of a code's life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PairOp {
    /// Keep `code` waiting for `ttl_secs`, for the agent that polls with `device_code` and
    /// asks to be enrolled as `ask`.
    Start {
        /// The code a person confirms.
        code: String,
        /// The secret the agent polls with.
        device_code: String,
        /// What the agent asks to be enrolled as, as the gate reads it.
        ask: Value,
        /// Seconds the code lasts.
        ttl_secs: u64,
    },
    /// What `code` asks for, while it waits for a confirm.
    Show {
        /// The code.
        code: String,
    },
    /// Take `code` from waiting to enrolling, once, for the confirm that enrolls it.
    Claim {
        /// The code.
        code: String,
    },
    /// End the enroll `claim` began: the controller's answer for the agent, or none to put
    /// the code back to waiting.
    Settle {
        /// The code.
        code: String,
        /// The enrollment, when it worked.
        enrolled: Option<Value>,
    },
    /// What the agent polling with `device_code` gets; an enrollment is handed out once.
    Poll {
        /// The secret the agent polls with.
        device_code: String,
    },
}
