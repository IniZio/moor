//! A start that asks for no setup kept from earlier Boxes.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::{BoxId, StartReq, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Warns when a start of a Box that exists asks to go cold, which only a new Box can.
pub(super) fn warn_ignored(cx: &Cx, req: &StartReq) {
    if req.cold {
        cx.warn(Warning {
            code: "cold_ignored".to_owned(),
            message: format!(
                "Box {} exists already, so it keeps its disk",
                req.name.as_str()
            ),
        });
    }
}

/// Records that new Box `id` goes cold, for whichever start provisions it, when `req` asks.
pub(super) fn record(store: &Store, req: &StartReq, id: BoxId) -> Result<(), CommandError> {
    if req.cold {
        store.apply(Decision::new(id).event(ExtraEvent::ColdAsked))?;
    }
    Ok(())
}
