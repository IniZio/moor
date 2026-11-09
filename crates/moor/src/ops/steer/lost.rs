//! A steer to a Box that lost its machine, which needs new hands first.

use moor_core::domain::hands::HandsState;
use moor_core::StartReq;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::start;
use crate::store::BoxRow;

/// Whether Box `row` lost its machine, so only new hands can take a steer.
pub(super) fn lost(row: &BoxRow) -> bool {
    row.hands
        .as_ref()
        .is_some_and(|h| h.state == HandsState::Gone)
}

/// Brings Box `row` back on new hands with its agent, as a start on it does.
pub(super) fn bring_back(cx: &Cx, row: &BoxRow) -> Result<(), CommandError> {
    let req = StartReq {
        name: row.name.clone(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        cold: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
    };
    start::run(cx, &req).map(drop)
}
