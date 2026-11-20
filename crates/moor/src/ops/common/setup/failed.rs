//! What a failed setup reports beyond its exit: the line it died at and a stopped container.

use moor_core::config::BoxSettings;
use moor_core::BoxHandle;

use super::{failed_step, FORCED_CLOSE};
use crate::cx::Cx;

/// Why the machine lists the Box's container stopped, if it does; ssh's own words prove nothing.
pub(crate) fn container(cx: &Cx, handle: &BoxHandle) -> Option<String> {
    if cx.env().cancel().is_cancelled() {
        return None;
    }
    let listing = cx
        .driver()
        .list_handles(std::slice::from_ref(handle))
        .ok()?;
    let held = listing.listed.into_iter().find(|l| &l.handle == handle)?;
    held.down.filter(|d| !d.is_empty())
}

/// The setup line a failed run died at, or the forced close that stopped it.
pub(super) fn line(cx: &Cx, cfg: &BoxSettings, (log, running): (&str, u32)) -> String {
    if cx.env().cancel().is_cancelled() {
        return FORCED_CLOSE.to_owned();
    }
    let at = failed_step(log, running, cfg.setup.len());
    cfg.setup.get(at).cloned().unwrap_or_default()
}
