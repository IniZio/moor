//! What a sweep of a machine's closed Boxes came to.

use serde::{Deserialize, Serialize};

/// An agent's answer to [`super::ops::Op::Sweep`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swept {
    /// The Boxes whose folders are gone.
    pub removed: Vec<i64>,
    /// The Boxes whose folders stay, to be tried again by the next sweep.
    pub errors: Vec<Failed>,
}

/// One Box a sweep could not clean up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failed {
    /// The Box.
    pub box_id: i64,
    /// Why.
    pub reason: String,
}
