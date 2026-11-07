//! A Box whose setup lines ran again over a checkpoint of another commit: what it records about
//! that disk, and the one start from scratch it gets when a line fails there.

use moor_core::config::BoxSettings;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::domain::layers::WarmDisk;
use moor_core::view::WorkTreeView;
use moor_core::{BoxId, Warning};
use serde::Deserialize as _;

use super::acquired::record_acquired;
use super::clone::Cloned;
use super::failed::failed;
use super::layers::{self, Layered};
use super::lease::{hold_root, hold_workdir};
use super::progress::acquire_timed;
use super::{reread, run_and_record, Entry, Provisioned, SetupTarget};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::setup::SetupRun;
use crate::store::lease::LeaseKind;
use crate::store::Store;

/// Whether the refresh fell back to a fresh clone, which replaced the old tree and its ignored
/// files.
const fn tree_went_cold(cloned: &Cloned) -> bool {
    matches!(
        cloned.work_tree,
        Some(WorkTreeView::Cold | WorkTreeView::ColdAfter { .. })
    )
}

/// What `target` goes on with after its clone, and the commit of the disk its lines run again
/// over when they do. A checkpoint records its commit, so only a tree that is the base unchanged
/// keeps its layers. A fork whose refresh fell back to a fresh clone lost the outputs its lines
/// made, so it keeps none and all its lines run.
pub(super) fn after_clone<'a>(
    target: &SetupTarget<'a>,
    cloned: &Cloned,
) -> (SetupTarget<'a>, Option<&'a str>) {
    let forked = target.layered.and_then(Layered::forked_commit);
    let keep = cloned.at_base && !(forked.is_some() && tree_went_cold(cloned));
    let target = target.keeping_layers(keep);
    let reran = forked.filter(|_| target.layered.is_none_or(Layered::reruns));
    (target, reran)
}

/// Records the disk commit `reran` the Box's lines run again over, when they do. A tree that went
/// cold has nothing stale left to name.
pub(super) fn record_disk(
    store: &Store,
    id: BoxId,
    reran: Option<&str>,
    cloned: &Cloned,
) -> Result<Option<WarmDisk>, CommandError> {
    let Some(commit) = reran.filter(|_| !tree_went_cold(cloned)) else {
        return Ok(None);
    };
    let (changed, ignored) = cloned.disk.clone();
    let disk = WarmDisk {
        commit: commit.to_owned(),
        changed,
        ignored,
    };
    store.apply(Decision::new(id).event(ExtraEvent::WarmRerun {
        commit: disk.commit.clone(),
        changed: disk.changed,
        ignored: disk.ignored.clone(),
    }))?;
    Ok(Some(disk))
}

/// What the agent of Box `id` is told about the disk its machine forked; none unless the Box's
/// current machine reran its lines over a checkpoint of another commit.
pub(crate) fn warm_disk_note(store: &Store, id: BoxId) -> Option<String> {
    let rerun = store.newest_events_of(id, "warm_rerun", 1).ok()?.pop()?;
    let machine = store.newest_events_of(id, "box_requested", 1).ok()?.pop();
    if machine.is_some_and(|m| m.id > rerun.id) {
        return None;
    }
    let disk = WarmDisk::deserialize(&rerun.payload).ok()?;
    Some(disk.note())
}

/// Gives up the fork `target` holds after line run `run` failed on it, then sets the Box up once
/// more on a new machine. The failure may come from what the old disk held, so a line that also
/// fails there is the one the Box reports.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, its store, whether it advances, its fork, layers, settings and failed run"
)]
pub(super) fn cold_again(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    layered: &Layered,
    cfg: &BoxSettings,
    run: &SetupRun,
) -> Result<Provisioned, CommandError> {
    let seen = run.view();
    let line = seen.line.unwrap_or_default();
    cx.warn(Warning {
        code: "warm_rerun_failed".to_owned(),
        message: format!(
            "setup line `{line}` failed over a checkpoint of another commit; setup runs from its \
             start on a new machine"
        ),
    });
    let failed_rerun = ExtraEvent::WarmRerunFailed {
        line,
        tail: seen.tail.unwrap_or_default(),
    };
    store.apply(Decision::new(id).event(failed_rerun))?;
    store.unpin_checkpoints(id)?;
    layers::release_old(cx, store, id, target.handle)?;
    for lease in store.leases(id)? {
        if lease.kind == LeaseKind::BoxDir {
            store.drop_lease(lease.id)?;
        }
    }
    let row = reread(store, id)?;
    let req = layered.machine_req(cx, store, &row, cfg)?;
    let planned = cx.driver().planned_handle(&req);
    let root = store.plan_lease(id, LeaseKind::BoxRoot, planned.as_str())?;
    store.apply(Decision::new(id).box_input(BoxInput::DriverMissing))?;
    let begin = Decision::new(id)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_handle: Field::Clear,
            ..Fields::default()
        })
        .intent(IntentChange::Set(Intent::Provisioning {
            pid: std::process::id(),
        }));
    store.apply(begin)?;
    let got = match acquire_timed(cx, store, id, &req) {
        Ok(got) => got,
        Err(e) => {
            let entry = if advance { Entry::Retry } else { Entry::Stay };
            return Err(failed(cx, store, id, entry, &req, e));
        }
    };
    hold_root(store, id, root, &planned, &got)?;
    record_acquired(store, id, &got, None)?;
    hold_workdir(store, id, &got)?;
    let fresh = layers::plan(cx, store, &row, target.provider, cfg, layered.cache());
    let next = SetupTarget::new(
        &got,
        target.provider,
        target.repo,
        target.log_dir,
        fresh.as_ref(),
    );
    run_and_record(cx, store, id, advance, &next, cfg)
}
