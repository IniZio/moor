//! Building a start request.

use super::{Placement, StartReq};
use crate::name::BoxName;

impl StartReq {
    /// A start of Box `name` that asks for nothing more: no repo, prompt or node, and every flag off.
    #[must_use]
    pub const fn named(name: BoxName) -> Self {
        Self {
            name,
            prompt: None,
            issue: None,
            base: None,
            model: None,
            node: None,
            placement: Placement::FirstOnly,
            place: false,
            no_agent: false,
            cold: false,
            resume: false,
            repo: None,
            also: None,
            config: None,
            parent: None,
            gates: None,
            identity: None,
        }
    }
}
