//! A start's gates: checked before any write, and kept with the Box.

use moor_core::gate::Gates;
use moor_core::{ErrorKind, StartReq};

use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Refuses gates that are not valid, so a bad set fails when it is given.
///
/// # Errors
/// Usage for the first thing wrong with the gates.
pub(super) fn check(req: &StartReq) -> Result<(), CommandError> {
    let Some(docs) = &req.gates else {
        return Ok(());
    };
    Gates::parse(docs)
        .map(drop)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))
}

/// The JSON a Box keeps of the gates `req` asks for; `None` for none.
pub(super) fn column(req: &StartReq) -> Option<String> {
    req.gates
        .as_ref()
        .and_then(|docs| serde_json::to_string(docs).ok())
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
        Gates::parse(docs).map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
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
