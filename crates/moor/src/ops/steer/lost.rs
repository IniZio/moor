//! A steer to a Box that lost its machine, which needs new hands first.

use moor_core::domain::hands::HandsState;
use moor_core::{ErrorDetail, ErrorKind, StartReq};

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
        place: false,
        gates: None,
        placement: moor_core::decide::schedule::Placement::FirstOnly,
        cold: false,
        config: None,
        name: row.name.clone(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        no_setup: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
        task: None,
    };
    start::run(cx, &req).map(drop)
}

/// The refusal for an agent stopped at a first-run screen: a key typed there may pick an option
/// that ends the agent, so the Box is started again instead.
pub(super) fn first_run_refusal(name: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        format!(
            "the agent of {name} stopped at a first-run screen that moor's pre-seed should have skipped; this is a moor bug; end the Box with `moor end {name}` and start it again"
        ),
    )
    .with_detail(ErrorDetail::FirstRunScreen)
}
