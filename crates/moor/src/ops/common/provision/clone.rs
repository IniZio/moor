//! The clone of a provisioned Box's repo, run in the Box after its Proxy is ready.

use std::path::PathBuf;
use std::time::Instant;

use moor_core::boxscript::clone::{clone_script, Checkout, CloneStep};
use moor_core::boxscript::refresh::{self, refresh_script};
use moor_core::boxscript::resources;
use moor_core::config::BoxSettings;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Output;
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::shape::Shape;
use moor_core::{box_key, BoxId, ProviderId, Warning};

use super::outcome::clone_failed;
use super::SetupTarget;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, BoxProxy, Site};
use crate::ops::common::cache;
use crate::ops::common::dotfiles;
use crate::ops::common::identity::Identity;
use crate::ops::common::proxy::TAIL_LINES;
use crate::store::Store;

/// What the clone exec reported about the Box.
#[derive(Default)]
pub(super) struct Cloned {
    /// The clone's milliseconds; none when the Box has no GitHub repo to clone.
    pub(super) ms: Option<u64>,
    /// The CPUs and memory the Box has, as read inside it.
    pub(super) delivered: Option<Shape>,
}

/// The Proxy setup and dotfiles run through, or why it could not start; `None` when nothing needs it.
pub(super) type SetupProxy = Result<Option<BoxProxy>, String>;

/// The clone the Box runs: its URL, base and branch.
struct Plan {
    /// `https://github.com/OWNER/NAME.git`.
    url: String,
    /// The commit the Box starts from.
    base: String,
    /// The Box branch.
    branch: String,
    /// The Box's key when it keeps its work tree, which is refreshed once per Box before the clone.
    refresh: Option<String>,
}

impl Plan {
    /// The values the clone script is built from.
    fn checkout(&self) -> Checkout<'_> {
        Checkout {
            url: &self.url,
            base: &self.base,
            branch: &self.branch,
        }
    }
}

/// Starts the Proxy when the clone, setup or dotfiles need it, then clones the repo in the Box with
/// the Proxy env and sets the Box's git identity in the same exec.
///
/// Returns the clone's milliseconds, none when the Box has no GitHub repo to clone, and the CPUs
/// and memory the same exec read. github.com is
/// reached only through the Proxy, which places the credential, so the clone never starts before
/// the Proxy is ready. A failed clone aborts provisioning and stops a Proxy this call started.
pub(super) fn clone_with_proxy(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    advance: bool,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<(Cloned, SetupProxy), CommandError> {
    let plan = plan(cx, store, id, target, cfg)?;
    let wants = plan.is_some() || !cfg.setup.is_empty() || cfg.dotfiles.is_some();
    let was_up = wants && matches!(cx.proxy_host().status(id), Ok(ProxyStatus::Ready { .. }));
    let proxy = wants
        .then(|| {
            box_exec::proxy(
                cx,
                id,
                Some(target.handle.clone()),
                target.repo.map(|r| &r.repo),
                cfg,
            )
        })
        .transpose();
    let job = Job::new(id, target, plan.as_ref());
    let result = match (&plan, &proxy) {
        (Some(_), Err(reason)) => Err(format!("the Proxy could not start: {reason}")),
        _ if job.is_empty() => return Ok((Cloned::default(), proxy)),
        _ => clone(cx, target, cfg, &job, &proxy),
    };
    match result {
        Ok((ms, delivered)) => {
            let ms = plan.map(|_| ms);
            Ok((Cloned { ms, delivered }, proxy))
        }
        Err(reason) => {
            if matches!(proxy, Ok(Some(_))) && !was_up {
                if let Err(e) = cx.proxy_host().stop(id) {
                    cx.warn(Warning {
                        code: "proxy_stop_failed".to_owned(),
                        message: format!("{e}; the Box is kept anyway"),
                    });
                }
            }
            let reason = format!("cloning the repo into the Box failed: {reason}");
            Err(clone_failed(cx, store, id, advance, target.handle, reason))
        }
    }
}

/// What the Box clones: its repo at the Box's base and on its branch; none for a Box with no repo.
fn plan(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
) -> Result<Option<Plan>, CommandError> {
    let Some(repo) = target.repo else {
        return Ok(None);
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(None);
    };
    let provider = ProviderId::new(target.provider);
    let refresh = cache::keeps_work_tree(cx, store, &row, cfg, &provider)
        .then(|| target.handle.as_str().to_owned());
    Ok(Some(Plan {
        url: repo.repo.clone_url(),
        base: row.base.unwrap_or_else(|| "HEAD".to_owned()),
        branch: row.branch.unwrap_or_else(|| box_key::branch(&row.name)),
        refresh,
    }))
}

/// What the clone exec runs for Box `id`: the clone and the identity lines of the repo's author.
struct Job<'a> {
    /// The Box.
    id: BoxId,
    /// The clone, when the Box has a GitHub repo.
    plan: Option<&'a Plan>,
    /// The identity to set.
    identity: Option<Identity>,
}

impl<'a> Job<'a> {
    /// Whether the Box has no repo to clone and no identity to set, so no exec runs and the probe
    /// has none to ride.
    const fn is_empty(&self) -> bool {
        self.plan.is_none() && self.identity.is_none()
    }

    /// The job for Box `id`, with the author stored with the target's repo.
    fn new(id: BoxId, target: &SetupTarget<'_>, plan: Option<&'a Plan>) -> Self {
        let identity = target.repo.and_then(|r| Identity::of(&r.author));
        Self { id, plan, identity }
    }
}

/// Runs the job's identity lines, clone script and resource probe as one logged exec; returns its
/// milliseconds and the shape the probe read, or the reason it failed with the log's tail.
///
/// The probe runs after the clone and the exec still exits with the clone's code, so a failed clone
/// reports as before.
fn clone(
    cx: &Cx,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    job: &Job<'_>,
    proxy: &SetupProxy,
) -> Result<(u64, Option<Shape>), String> {
    let log = clone_log(cx, job.id)
        .inspect_err(|e| {
            if job.identity.is_some() {
                cx.warn(Warning {
                    code: "identity_failed".to_owned(),
                    message: format!("could not set the git identity in the box: {e}"),
                });
            }
        })
        .ok();
    let plan = job.plan;
    let identity = job.identity.as_ref().filter(|_| log.is_some());
    if plan.is_none() && identity.is_none() {
        return Ok((0, None));
    }
    let mut script = identity.map(Identity::script).unwrap_or_default();
    if let Some(plan) = plan {
        if let Some(key) = &plan.refresh {
            script.push_str(&refresh_script(&plan.checkout(), key));
        }
        script.push_str(&clone_script(&plan.checkout()));
    }
    let script = resources::after(&script);
    let site = Site {
        handle: target.handle,
        provider: target.provider,
        workdir: Some(target.workdir),
    };
    let proxy = proxy.as_ref().ok().and_then(Option::as_ref);
    let exec = box_exec::prepare(cx, &site, proxy, cfg, Purpose::Clone)?;
    // A logged exec streams, and only a streamed exec turns SIGINT into an outcome, so the
    // Proxy started for it is still stopped.
    let spec = ExecSpec {
        argv: exec.argv(vec!["sh".to_owned(), "-c".to_owned(), script]),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: log.clone().map(Output::Log),
    };
    let clock = cx.clock();
    let started = clock.instant();
    let outcome = cx.driver().exec(target.handle, &spec, exec.deadline());
    let ms = millis(started, clock.instant());
    let text = log.as_deref().map(|log| {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let _ = std::fs::remove_file(log);
        text
    });
    let reason = match outcome {
        Ok(ExecOutcome::Exited { code: 0 }) => {
            let delivered = text.as_deref().and_then(resources::parse);
            report(cx, identity, plan, text.as_deref());
            return Ok((ms, delivered));
        }
        Ok(ExecOutcome::Exited { code }) => CloneStep::of_exit(code)
            .zip(plan)
            .map_or_else(|| format!("exit {code}"), |(s, p)| s.reason(&p.checkout())),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    Err(with_tail(reason, text.as_deref().unwrap_or_default()))
}

/// Reports what a passing clone exec printed in `text`: a failed identity key, and how a kept work
/// tree started.
fn report(cx: &Cx, identity: Option<&Identity>, plan: Option<&Plan>, text: Option<&str>) {
    if let (Some(identity), Some(text)) = (identity, text) {
        identity.report(cx, text);
    }
    let kept = plan.is_some_and(|p| p.refresh.is_some());
    if let Some(how) = text.filter(|_| kept).and_then(refresh::outcome) {
        cx.warn(Warning {
            code: "work_tree".to_owned(),
            message: format!("work tree {how}"),
        });
    }
}

/// `reason`, then the last lines of the exec's output that are not the identity's exit lines.
fn with_tail(reason: String, text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            !l.trim().is_empty() && !l.starts_with("user.name=") && !l.starts_with("user.email=")
        })
        .collect();
    let tail = lines
        .get(lines.len().saturating_sub(TAIL_LINES)..)
        .unwrap_or_default();
    if tail.is_empty() {
        return reason;
    }
    format!("{reason}\n{}", tail.join("\n"))
}

/// A private host file in the state dir for the clone exec's output of Box `id`.
fn clone_log(cx: &Cx, id: BoxId) -> Result<PathBuf, String> {
    let dir = cx.state_dir().map_err(|e| e.error.to_string())?;
    let log = dir.join("clone").join(format!("{}.log", id.get()));
    dotfiles::private_log(&log).map_err(|e| format!("could not create the clone log: {e}"))?;
    Ok(log)
}

/// Whole milliseconds from `from` to `to`.
fn millis(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}
