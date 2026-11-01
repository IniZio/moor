//! Runs a Box's setup lines through the Proxy and records each line as it reports.

use std::sync::{Mutex, PoisonError};

use moor_core::config::BoxSettings;
use moor_core::BoxId;

use super::clone::SetupProxy;
use super::SetupTarget;
use crate::cx::Cx;
use crate::ops::common::{setup, step};
use crate::store::Store;

/// Runs the setup lines through the Proxy; a Proxy that will not start fails the setup.
pub(super) fn run_through_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> setup::SetupRun {
    match proxy {
        Ok(proxy) => {
            // The lines are reported from a thread that follows the script's output, which
            // cannot hold the `Cx`; the first write failure is warned once the script ends.
            let unrecorded = Mutex::new(None);
            let run = setup::run_setup(
                cx,
                target.handle,
                target.provider,
                target.workdir,
                target.log_dir,
                cfg,
                proxy.as_ref(),
                &|event| {
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
        Err(_) if cfg.setup.is_empty() => setup::SetupRun::Skipped,
        Err(reason) => {
            let name = store.box_row(id).ok().flatten().map(|r| r.name);
            setup::proxy_failed(cx, name.as_ref().map_or("<box>", |n| n.as_str()), reason)
        }
    }
}
