//! Saving a Box's layers: the checks a disk must pass, and the row that says what it holds.

use std::sync::atomic::Ordering;
use std::time::Duration;

use moor_core::boxscript::warm_disk;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::Warning;

use super::Layered;
use crate::cx::Cx;
use crate::ops::common::provision::disk::make_room;
use crate::ops::common::provision::SetupTarget;
use crate::store::checkpoints::Place;
use crate::store::Store;

/// How long reading the Box's HEAD may take.
const HEAD_CHECK_DEADLINE: Deadline = Deadline(Duration::from_mins(1));

/// Checkpoints the agent install and profile the target's Box finished in `ms`, once they all went
/// well and its tree is still the base.
pub(in super::super) fn after_finishing(cx: &Cx, store: &Store, target: &SetupTarget<'_>, ms: u64) {
    let Some(layered) = target.layered.filter(|l| !l.finished) else {
        return;
    };
    layered.spent_ms.fetch_add(ms, Ordering::Relaxed);
    save(cx, store, target, layered, layered.keys.len());
}

/// Saves `layer` of a rerun over the one kept for it, at the commit it holds now, once the tree is
/// that commit with no changes. The old one stays until nothing depends on it.
pub(in super::super) fn roll_forward(
    cx: &Cx,
    store: &Store,
    target: &SetupTarget<'_>,
    layered: &Layered,
    layer: usize,
) {
    save(cx, store, target, layered, layer);
}

/// Whether the target's HEAD is still the Box's base, which is the commit a checkpoint records. A
/// line may have moved it since the clone said so.
fn head_at_base(cx: &Cx, target: &SetupTarget<'_>, layered: &Layered) -> bool {
    let check = if layered.roll.is_some() {
        warm_disk::tree_at(&layered.base)
    } else {
        warm_disk::head_at(&layered.base)
    };
    let spec = ExecSpec {
        argv: ["sh", "-c", &check].map(str::to_owned).to_vec(),
        env: Vec::new(),
        cwd: Some(target.workdir.clone()),
        output: None,
    };
    let ended = cx.driver().exec(target.handle, &spec, HEAD_CHECK_DEADLINE);
    matches!(ended, Ok(ExecOutcome::Exited { code: 0 }))
}

/// Checkpoints `layer` of the target's Box, unless a checkpoint of it is kept already.
/// A failed checkpoint is only warned about: setup goes on, and a retry runs the layer again.
pub(super) fn save(
    cx: &Cx,
    store: &Store,
    target: &SetupTarget<'_>,
    layered: &Layered,
    layer: usize,
) {
    let Some(key) = layered.key_at(layer) else {
        return;
    };
    let replace = layered.replacing || layered.roll.is_some();
    if !replace && store.use_checkpoint(&layered.home, key).unwrap_or(false) {
        return;
    }
    let refused = if layer > 0 && !head_at_base(cx, target, layered) {
        Some("a setup line moved HEAD off the base commit or left changes, or moor could not tell")
    } else if !make_room(cx, store, layered.id, &layered.home).unwrap_or(true) {
        Some("the disk is below its floor with no checkpoint left to delete")
    } else {
        None
    };
    if let Some(why) = refused {
        cx.warn(Warning {
            code: "checkpoint_skipped".to_owned(),
            message: format!("did not checkpoint setup layer {layer}: {why}"),
        });
        return;
    }
    let began = cx.clock().now().as_millis();
    let taken = cx.driver().checkpoint(target.handle, key.as_str());
    let duration_ms = u64::try_from(cx.clock().now().as_millis() - began).unwrap_or_default();
    let told = ExtraEvent::CheckpointTaken {
        layer: u32::try_from(layer).unwrap_or(u32::MAX),
        key: key.as_str().to_owned(),
        checkpoint: taken.as_ref().ok().map(|c| c.id.clone()),
        duration_ms,
        error: taken.as_ref().err().map(ToString::to_string),
    };
    if let Err(e) = store.apply(Decision::new(layered.id).event(told)) {
        cx.warn(Warning {
            code: "checkpoint_unrecorded".to_owned(),
            message: format!("could not record the checkpoint of setup layer {layer}: {e}"),
        });
    }
    let saved = taken.map_err(|e| e.to_string()).and_then(|made| {
        let place = Place {
            workdir: target.workdir.clone(),
            setup_ms: layered.spent_ms.load(Ordering::Relaxed),
            base: layered.base.clone(),
            stale_since: layered.saved_stale_since(),
        };
        store
            .record_checkpoint(&layered.home, key, &layered.lineage_at(store.now()), &made)
            .and_then(|()| store.place_checkpoint(&layered.home, key, &place))
            .and_then(|()| store.pin_checkpoint(layered.id, &layered.home, key, layer))
            .map_err(|e| e.to_string())
    });
    if let Err(e) = saved {
        cx.warn(Warning {
            code: "checkpoint_failed".to_owned(),
            message: format!(
                "could not checkpoint setup layer {layer}: {e}; a retry runs it again"
            ),
        });
    }
}
