//! A menu choice that carries text: the choice that opens a free-text field, then the words.

use std::time::Duration;

use moor_core::seam::engine::{SessionRef, Steer};
use moor_core::SteerReq;

use super::send;
use crate::cx::Cx;
use crate::error::CommandError;

/// What the steer is recorded as having sent.
pub(super) fn sent_of(steer: &Steer, req: &SteerReq) -> String {
    match (steer, req.text.as_deref().filter(|_| req.menu.is_some())) {
        (Steer::Text(t), _) => t.clone(),
        (Steer::Menu(n), Some(typed)) => format!("{n}: {typed}"),
        (Steer::Menu(n), None) => n.to_string(),
        (Steer::Interrupt, _) => "(interrupt)".to_owned(),
    }
}

/// How long the agent gets to open its text field after a menu choice that asks for one.
const FIELD_OPENS: Duration = Duration::from_millis(400);

/// Types the text of a menu-and-text steer once the menu choice is in: the choice that opens a
/// free-text field, then what the person wrote in it.
pub(super) fn type_after_menu(
    cx: &Cx,
    sref: &SessionRef,
    req: &SteerReq,
    steer: &Steer,
) -> Result<(), CommandError> {
    let (Steer::Menu(_), Some(typed)) = (steer, req.text.as_deref().filter(|_| req.menu.is_some()))
    else {
        return Ok(());
    };
    cx.clock().sleep(FIELD_OPENS);
    send(cx, sref, &Steer::Text(typed.to_owned()))?;
    Ok(())
}
