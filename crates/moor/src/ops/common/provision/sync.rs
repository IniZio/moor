//! The checkout sync of a provisioned Box, with the Proxy started beside it.

use std::path::PathBuf;
use std::time::Instant;

use moor_core::config::BoxSettings;
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::{BoxId, Warning};

use super::outcome::sync_failed;
use super::SetupTarget;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, BoxProxy};
use crate::ops::common::dotfiles;
use crate::ops::common::identity::Identity;
use crate::ops::common::setup;
use crate::store::Store;

/// The Proxy setup and dotfiles run through, or why it could not start; `None` when neither has work.
pub(super) type SetupProxy = Result<Option<BoxProxy>, String>;

/// Syncs the checkout into the Box while the Proxy starts, and returns the sync's milliseconds.
///
/// The sync exec also sets the Box's git identity. One Proxy serves setup and dotfiles, so it
/// starts only when one of them has work. A failed sync aborts provisioning with the sync's error
/// whatever the Proxy did, and stops a Proxy this call started.
pub(super) fn sync_with_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<(u64, SetupProxy), CommandError> {
    let wants = !cfg.setup.is_empty() || cfg.dotfiles.is_some();
    let was_up = wants && matches!(cx.proxy_host().status(id), Ok(ProxyStatus::Ready { .. }));
    let identity = Identity::of(cx, target.worktree);
    let log = sync_log(cx, id)
        .inspect_err(|e| {
            if identity.is_some() {
                cx.warn(Warning {
                    code: "identity_failed".to_owned(),
                    message: format!("could not set the git identity in the box: {e}"),
                });
            }
        })
        .ok();
    let identity = identity.filter(|_| log.is_some());
    let script = identity.as_ref().map(Identity::script).unwrap_or_default();
    // A logged exec streams, and only a streamed exec turns SIGINT into an outcome, so the
    // Proxy started beside it is still stopped.
    let prelude = log.as_deref().map(|log| setup::Prelude {
        script: &script,
        log,
    });
    let (driver, clock) = (cx.driver(), cx.clock());
    let (synced, proxy) = std::thread::scope(|scope| {
        let sync = scope.spawn(|| {
            let started = clock.instant();
            let (handle, workdir, worktree) = (target.handle, target.workdir, target.worktree);
            setup::sync_checkout(driver, handle, workdir, worktree, cfg, prelude.as_ref())
                .map(|()| millis(started, clock.instant()))
        });
        let proxy = wants
            .then(|| box_exec::proxy(cx, id, Some(target.handle.clone()), target.worktree, cfg))
            .transpose();
        let synced = sync.join().unwrap_or_else(|p| std::panic::resume_unwind(p));
        (synced, proxy)
    });
    let text = log.as_deref().map(|log| {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let _ = std::fs::remove_file(log);
        text
    });
    let reason = match synced {
        Ok(ms) => {
            if let (Some(identity), Some(text)) = (&identity, &text) {
                identity.report(cx, text);
            }
            return Ok((ms, proxy));
        }
        Err(reason) => reason,
    };
    if matches!(proxy, Ok(Some(_))) && !was_up {
        if let Err(e) = cx.proxy_host().stop(id) {
            cx.warn(Warning {
                code: "proxy_stop_failed".to_owned(),
                message: format!("{e}; the Box is kept anyway"),
            });
        }
    }
    Err(sync_failed(cx, store, id, advance, target.handle, reason))
}

/// A private host file in the state dir for the sync exec's output of Box `id`.
fn sync_log(cx: &Cx, id: BoxId) -> Result<PathBuf, String> {
    let dir = cx.state_dir().map_err(|e| e.error.to_string())?;
    let log = dir.join("sync").join(format!("{}.log", id.get()));
    dotfiles::private_log(&log).map_err(|e| format!("could not create the sync log: {e}"))?;
    Ok(log)
}

/// Whole milliseconds from `from` to `to`.
fn millis(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}
