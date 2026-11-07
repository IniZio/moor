//! Runs a Box's setup lines through the Proxy and records each line as it reports.

use std::sync::{Mutex, PoisonError};

use moor_core::config::BoxSettings;
use moor_core::domain::decision::ExtraEvent;
use moor_core::BoxId;

use super::clone::SetupProxy;
use super::others::named;
use super::SetupTarget;
use crate::cx::Cx;
use crate::ops::common::setup::{self, Released, SetupRun};
use crate::ops::common::step;
use crate::store::Store;

/// Runs the setup lines through the Proxy; a Proxy that will not start fails the setup.
///
/// `place` is the position from 1 and the total of a single line run apart from the rest, which
/// its events report in place of their own.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, its store, where setup runs, its settings, Proxy and the line's place"
)]
pub(super) fn run_through_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
    place: Option<(u32, u32)>,
) -> Result<SetupRun, Released> {
    match proxy {
        Ok(proxy) => {
            // The lines are reported from a thread that follows the script's output, which
            // cannot hold the `Cx`; the first write failure is warned once the script ends.
            let unrecorded = Mutex::new(None);
            let attempt = store
                .newest_events_of(id, "setting_up", 1)
                .ok()
                .and_then(|mut e| e.pop())
                .map_or(0, |e| e.id);
            let run = setup::run_setup(
                cx,
                target.handle,
                (
                    target.provider,
                    setup::run_id(attempt, target.workdir, place),
                ),
                target.workdir,
                target.log_dir,
                cfg,
                proxy.as_ref(),
                &|event| {
                    let event = named(placed(event, place), target.tree_of);
                    if let Err(e) = step::try_record(store, id, event) {
                        unrecorded
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .get_or_insert(e);
                    }
                },
            );
            if let Some(e) = unrecorded
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner)
            {
                step::warn_unrecorded(cx, &e);
            }
            run
        }
        // Dotfiles report their own Proxy failure; no setup line was meant to run.
        Err(_) if cfg.setup.is_empty() => Ok(SetupRun::Skipped),
        Err(reason) => {
            let name = store.box_row(id).ok().flatten().map(|r| r.name);
            Ok(setup::proxy_failed(
                cx,
                name.as_ref().map_or("<box>", |n| n.as_str()),
                reason,
            ))
        }
    }
}

/// `event` of a line run apart, at `place` among all the lines.
fn placed(event: ExtraEvent, place: Option<(u32, u32)>) -> ExtraEvent {
    let Some((index, total)) = place else {
        return event;
    };
    match event {
        ExtraEvent::SetupLineStarted { line, .. } => {
            ExtraEvent::SetupLineStarted { index, total, line }
        }
        ExtraEvent::SetupLineFinished { ms, ok, .. } => ExtraEvent::SetupLineFinished {
            index,
            total,
            ms,
            ok,
        },
        other => other,
    }
}
