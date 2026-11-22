//! A start's gates: checked before any write, and kept with the Box.

use moor_core::gate::Gates;
use moor_core::{ErrorKind, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::workflow::unreportable;
use crate::store::{BoxRow, Store};

/// Refuses gates that are not valid, so a bad set fails when it is given.
///
/// # Errors
/// Usage for the first thing wrong with the gates.
pub(super) fn check(req: &StartReq) -> Result<(), CommandError> {
    let Some(docs) = &req.gates else {
        return Ok(());
    };
    Gates::parse(docs, &[])
        .map(drop)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))
}

/// Refuses gates that a new Box could not honour: with no prompt and no issue the agent is never
/// told its gates, and on a controller that cannot offer `report_step` an agent could not hand in
/// evidence or ask its owner. Checked before any write, for a start that makes a new Box; a start
/// that attaches keeps the gates its Box has.
///
/// # Errors
/// Usage naming what is missing.
pub(super) fn check_fresh(cx: &Cx, req: &StartReq) -> Result<(), CommandError> {
    let Some(docs) = &req.gates else {
        return Ok(());
    };
    let usage = |why: String| CommandError::new(ErrorKind::Usage, why);
    let gates = Gates::parse(docs, &[]).map_err(|e| usage(e.to_string()))?;
    if req.prompt.is_none() && req.issue.is_none() {
        return Err(usage(
            "a start with gates needs a prompt or an issue: the agent is told its gates in its \
             first message"
                .to_owned(),
        ));
    }
    match gates
        .needs_reports()
        .then(|| unreportable(cx, "a gate with evidence or approval"))
    {
        Some(Some(why)) => Err(usage(why)),
        _ => Ok(()),
    }
}

/// The JSON a Box keeps of the gates `req` asks for; `None` for none.
pub(super) fn column(req: &StartReq) -> Option<String> {
    req.gates
        .as_ref()
        .and_then(|docs| Gates::parse(docs, &[]).ok())
        .and_then(|gates| serde_json::to_string(&gates.doc()).ok())
}

/// Refuses a start of open Box `row` that asks for gates other than the Box's own, rather than
/// dropping them: a Box keeps the gates it started with.
///
/// # Errors
/// Refused when `req` carries gates that differ from the Box's.
pub(super) fn check_kept(store: &Store, req: &StartReq, row: &BoxRow) -> Result<(), CommandError> {
    let Some(docs) = &req.gates else {
        return Ok(());
    };
    let asked =
        Gates::parse(docs, &[]).map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
    if store.box_gates(row.id)? == Some(asked) {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "open Box {} has the gates it started with, which a start does not change",
            row.name.as_str()
        ),
    )
    .with_hint("close the Box and start a new one to give it other gates"))
}
