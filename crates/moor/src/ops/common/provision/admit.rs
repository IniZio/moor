//! Admission: a new or waiting Box takes a free slot of its provider, waits for one, or, when
//! the start named its provider, is refused.

use moor_core::config::{BoxSettings, Layer};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::{BoxId, ErrorKind, ProviderId};

use super::Entry;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::capacity;
use crate::store::Store;

/// Why a start that named its provider is refused at the Box limit.
const NAMED_FULL: &str = "the Box limit is reached, and a Box on a provider named in the start \
                          cannot wait for a free slot: start it again once a Box closes";

/// Applies `requested` when `provider` has a free slot; true when the Box goes on to acquire.
///
/// # Errors
/// Refused when every slot is taken and the start named its provider; the Box is then
/// rejected, since a queued Box keeps no provider and a promotion would use `box.provider`.
pub(super) fn admit(
    cx: &Cx,
    store: &Store,
    (id, entry): (BoxId, Entry),
    cfg: &BoxSettings,
    provider: &ProviderId,
    requested: Decision,
) -> Result<bool, CommandError> {
    if !matches!(entry, Entry::Fresh | Entry::Waiting) {
        store.apply(requested)?;
        return Ok(true);
    }
    let named = entry == Entry::Fresh
        && cfg
            .provider
            .as_ref()
            .is_some_and(|p| p.source == Layer::Flag);
    let mut full = false;
    store.apply_admitting(provider, |holders| {
        let Some(lim) = capacity::full(cx.config(), provider, holders) else {
            return Some(requested);
        };
        full = true;
        (entry == Entry::Fresh).then(|| {
            let input = if named {
                BoxLifecycleInput::Rejected {
                    reason: NAMED_FULL.to_owned(),
                }
            } else {
                BoxLifecycleInput::AtCapacity {
                    max_boxes: lim.max.get(),
                }
            };
            Decision::new(id).lifecycle(input)
        })
    })?;
    if full && named {
        return Err(CommandError::new(ErrorKind::Refused, NAMED_FULL));
    }
    Ok(!full)
}
