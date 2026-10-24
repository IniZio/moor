//! A Box's cache volumes: the plan its acquire carries and the seal its close runs.
//!
//! Close derives the plan again from what start used: the `.moor.yaml` stored with the Box, the
//! operator config and the state dir's salt. So a close after a controller restart seals the
//! same volumes, and no plan is stored.

use std::time::Duration;

use moor_core::cache::{seal_failed, CachePlan, Origin};
use moor_core::config::{BoxSettings, DriverConfig};
use moor_core::seam::driver::{BoxStep, DriverCaps, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, ProviderId, Warning};

use crate::cx::Cx;
use crate::store::{BoxRow, Store};

/// Time allowed for the seal script; pruning a large Docker data root takes a while.
const SEAL_DEADLINE: Duration = Duration::from_mins(5);

/// Why a Box gets no cache volumes.
enum Cold {
    /// The operator turned them off for the provider, or the Box has no user.
    Off,
    /// The provider keeps none; a repo or the operator declared this many volumes.
    Unsupported(usize),
    /// The declarations clash, or the salt could not be read.
    Refused(String),
}

/// The plan for `row` on `provider`, or why the Box starts cold.
fn plan(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    provider: &ProviderId,
    caps: DriverCaps,
) -> Result<CachePlan, Cold> {
    let drivers = &cx.config().drivers;
    let on = drivers
        .get(provider.as_str())
        .is_none_or(DriverConfig::cache_volumes);
    let Some(user) = row.user.filter(|_| on) else {
        return Err(Cold::Off);
    };
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let refused = |e: &dyn std::fmt::Display| Cold::Refused(e.to_string());
    let declared = cfg
        .cache
        .declared(repo.is_some())
        .map_err(|e| refused(&e))?;
    if declared.is_empty() {
        return Ok(CachePlan::none());
    }
    if !caps.cache_volumes {
        let own = declared
            .iter()
            .filter(|d| d.origin != Origin::Preset)
            .count();
        return if own == 0 {
            Ok(CachePlan::none())
        } else {
            Err(Cold::Unsupported(own))
        };
    }
    let salt = store.cache_salt().map_err(|e| refused(&e))?;
    CachePlan::build(user, repo, &cfg.cache, &salt).map_err(|e| refused(&e))
}

/// The plan a new Box's acquire carries; a Box that gets none warns why, unless the operator
/// turned cache volumes off.
pub(crate) fn for_start(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    provider: &ProviderId,
    caps: DriverCaps,
) -> CachePlan {
    let (code, message) = match plan(cx, store, row, cfg, provider, caps) {
        Ok(plan) => return plan,
        Err(Cold::Off) => return CachePlan::none(),
        Err(Cold::Unsupported(n)) => (
            "cache_unavailable",
            format!(
                "provider {} keeps no cache volumes; {n} declared volumes skipped, \
                 the Box starts cold",
                provider.as_str()
            ),
        ),
        Err(Cold::Refused(why)) => ("cache_refused", format!("{why}; the Box starts cold")),
    };
    cx.warn(Warning {
        code: code.to_owned(),
        message,
    });
    CachePlan::none()
}

/// Runs the seal script of `row`'s plan in the Box, after teardown and before release. A volume
/// whose seal fails gets no marker, so the provider does not keep it; this only warns and never
/// fails the close.
pub(crate) fn seal(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    cfg: &BoxSettings,
    handle: &BoxHandle,
    provider: &ProviderId,
) {
    let Ok(caps) = cx.driver().caps(provider) else {
        return;
    };
    let script = plan(cx, store, row, cfg, provider, caps)
        .ok()
        .and_then(|p| p.seal_script());
    let Some(script) = script else {
        return;
    };
    let steps = [BoxStep::Run(script)];
    let done = cx
        .driver()
        .batch(handle, &[], &steps, Deadline(SEAL_DEADLINE));
    let message = match done.into_iter().next() {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => {
            let failed = seal_failed(&stdout);
            if failed.is_empty() {
                return;
            }
            format!(
                "the seal of cache volume {} failed, so it is not kept; moor emptied it in the \
                 Box, but partial contents may remain if that failed too",
                failed.join(", ")
            )
        }
        Some(Err(e)) => format!("the cache seal did not run, so no sealed volume is kept: {e}"),
        _ => "the cache seal did not finish, so no sealed volume is kept".to_owned(),
    };
    cx.warn(Warning {
        code: "cache_seal_failed".to_owned(),
        message: format!("{message}; the Box is released anyway"),
    });
}
