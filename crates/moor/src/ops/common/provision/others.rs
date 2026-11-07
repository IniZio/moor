//! The repos a Box lists after its primary one: each cloned into its own work tree and set up there.

use moor_core::boxscript::clone::{clone_script, Checkout, CloneStep};
use moor_core::boxscript::quote;
use moor_core::config::{parse_repo, BoxSettings};
use moor_core::domain::decision::{ExtraEvent, ListedRepo};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxId, BoxPath, Warning};

use super::clone::{clone_log_of, warn_left_out, with_tail, SetupProxy};
use super::layers;
use super::setup_run::run_through_proxy;
use super::SetupTarget;
use crate::cx::Cx;
use crate::ops::common::box_exec::{self, Site};
use crate::ops::common::setup::{Released, SetupRun};
use crate::store::Store;
use std::path::PathBuf;

/// A listed repo other than the primary one, and the work tree it gets beside the primary's.
pub(super) struct OtherTree<'a> {
    /// The repo.
    pub(super) listed: &'a ListedRepo,
    /// Its work tree: `<name>` in the directory that holds the primary's workdir.
    pub(super) workdir: BoxPath,
}

/// The work trees of every repo the target lists after its primary, in listed order; the reason
/// names both repos when two of them would share a tree.
pub(super) fn trees<'a>(target: &SetupTarget<'a>) -> Result<Vec<OtherTree<'a>>, String> {
    let Some(primary) = target.repo else {
        return Ok(Vec::new());
    };
    let root = target.workdir.as_str().trim_end_matches('/');
    let parent = root.rsplit_once('/').map_or("", |(p, _)| p);
    let mut taken = vec![(root.to_owned(), primary.repo.to_string())];
    let mut out = Vec::new();
    for listed in &primary.others {
        let dir = format!("{parent}/{}", listed.repo.name);
        let name = listed.repo.to_string();
        if let Some((_, other)) = taken.iter().find(|(d, _)| *d == dir) {
            return Err(format!(
                "{other} and {name} would both be cloned into {dir}"
            ));
        }
        let workdir = BoxPath::restore(&dir).map_err(|e| format!("{name}: {e}"))?;
        taken.push((dir, name));
        out.push(OtherTree { listed, workdir });
    }
    Ok(out)
}

/// Clones each other repo into its work tree on the Box `branch`, all at once; the reason names
/// every repo whose clone failed, in listed order.
pub(super) fn clone_others(
    cx: &Cx,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
    branch: &str,
) -> Result<(), String> {
    let jobs: Vec<(String, Result<CloneJob, String>)> = trees(target)?
        .into_iter()
        .map(|tree| {
            let at = SetupTarget {
                workdir: &tree.workdir,
                tree_of: Some(&tree.listed.repo),
                ..*target
            };
            let log = at
                .tree_of
                .and_then(|repo| clone_log_of(cx, id, &repo.name).ok());
            let job = prepare_clone(cx, &at, log, cfg, proxy, branch);
            (tree.listed.repo.to_string(), job)
        })
        .collect();
    let driver = cx.driver();
    let handle = target.handle;
    let results: Vec<(String, Result<String, String>)> = if jobs.len() < 2 {
        jobs.into_iter()
            .map(|(name, job)| (name, job.and_then(|job| run_clone(driver, handle, &job))))
            .collect()
    } else {
        std::thread::scope(|scope| {
            let mut running = Vec::new();
            for (name, job) in jobs {
                let handle = job.map(|job| scope.spawn(move || run_clone(driver, handle, &job)));
                running.push((name, handle));
            }
            running
                .into_iter()
                .map(|(name, handle)| {
                    let result = handle.and_then(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err("the clone thread panicked".to_owned()))
                    });
                    (name, result)
                })
                .collect()
        })
    };
    let failed: Vec<String> = results
        .into_iter()
        .filter_map(|(name, r)| match r {
            Ok(text) => {
                warn_left_out(cx, &format!("{name}:"), &text);
                None
            }
            Err(e) => Some(format!("{name}: {e}")),
        })
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// One other repo's clone, ready to run without the context.
struct CloneJob {
    /// The exec that clones.
    spec: ExecSpec,
    /// Its time limit.
    deadline: Deadline,
    /// The remote it clones.
    url: String,
    /// The Box branch it checks out.
    branch: String,
    /// The host file its output streams into.
    log: Option<PathBuf>,
}

/// The clone of the repo `at` holds, from its default branch when the remote has no Box branch.
fn prepare_clone(
    cx: &Cx,
    at: &SetupTarget<'_>,
    log: Option<PathBuf>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
    branch: &str,
) -> Result<CloneJob, String> {
    let url = at.tree_of.map(ForgeRepo::clone_url).unwrap_or_default();
    let checkout = Checkout {
        url: &url,
        base: "HEAD",
        branch,
    };
    let dir = quote(at.workdir.as_str());
    let script = format!(
        "mkdir -p -- {dir} && cd -- {dir} || exit {}\n{}",
        CloneStep::Clone as i32,
        clone_script(&checkout)
    );
    let site = Site {
        handle: at.handle,
        provider: at.provider,
        workdir: None,
    };
    let proxy = proxy.as_ref().ok().and_then(Option::as_ref);
    let exec = box_exec::prepare(cx, &site, proxy, cfg, Purpose::Clone)?;
    let spec = ExecSpec {
        argv: exec.argv(vec!["sh".to_owned(), "-c".to_owned(), script]),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: log.clone().map(Output::Log),
    };
    Ok(CloneJob {
        spec,
        deadline: exec.deadline(),
        url,
        branch: branch.to_owned(),
        log,
    })
}

/// Runs `job` and reads back, then removes, its log; returns what it printed.
fn run_clone(driver: &dyn Driver, handle: &BoxHandle, job: &CloneJob) -> Result<String, String> {
    let outcome = driver.exec(handle, &job.spec, job.deadline);
    let text = job
        .log
        .as_deref()
        .map(|log| {
            let text = std::fs::read_to_string(log).unwrap_or_default();
            let _ = std::fs::remove_file(log);
            text
        })
        .unwrap_or_default();
    let checkout = Checkout {
        url: &job.url,
        base: "HEAD",
        branch: &job.branch,
    };
    let reason = match outcome {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(text),
        Ok(ExecOutcome::Exited { code }) => {
            CloneStep::of_exit(code).map_or_else(|| format!("exit {code}"), |s| s.reason(&checkout))
        }
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    Err(with_tail(reason, &text))
}

/// Runs the primary repo's setup, then each other repo's in its own tree, stopping at the first
/// failure; a failed line of another repo is named `OWNER/NAME: <line>`.
pub(super) fn set_up_all(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    target: &SetupTarget<'_>,
    cfg: &BoxSettings,
    proxy: &SetupProxy,
) -> Result<SetupRun, Released> {
    let run = if target.layered.is_some() {
        layers::run(cx, store, id, target, cfg, proxy)?
    } else {
        run_through_proxy(cx, store, id, target, cfg, proxy, None)?
    };
    let trees = match trees(target) {
        Ok(trees) => trees,
        Err(e) if !run.failed() => return Ok(failed_before_run(cx, e)),
        Err(_) => return Ok(run),
    };
    let mut run = run;
    for tree in trees {
        if run.failed() {
            break;
        }
        let repo = &tree.listed.repo;
        let lines = match tree.listed.moor_yaml.as_deref() {
            None => continue,
            Some(text) => match parse_repo(&format!("{repo}:.moor.yaml"), text) {
                Ok(parsed) => parsed.setup,
                Err(e) => return Ok(failed_before_run(cx, format!("{repo}: {e}"))),
            },
        };
        let own = BoxSettings {
            setup: lines,
            ..cfg.clone()
        };
        let log_dir = target.log_dir.join(&repo.name);
        let at = SetupTarget {
            workdir: &tree.workdir,
            log_dir: &log_dir,
            tree_of: Some(repo),
            layered: None,
            ..*target
        };
        let next = run_through_proxy(cx, store, id, &at, &own, proxy, None)?;
        run = combined(run, next, &repo.to_string());
    }
    Ok(run)
}

/// `first` followed by `next` of repo `repo`, as one run.
fn combined(first: SetupRun, next: SetupRun, repo: &str) -> SetupRun {
    match (first, next) {
        (first, SetupRun::Skipped) => first,
        (first, SetupRun::Ok { ms, tail }) => SetupRun::Ok {
            ms: first.ms().unwrap_or_default() + ms,
            tail,
        },
        (
            first,
            SetupRun::Failed {
                line,
                exit,
                ms,
                tail,
            },
        ) => SetupRun::Failed {
            line: format!("{repo}: {line}"),
            exit,
            ms: first.ms().unwrap_or_default() + ms,
            tail,
        },
    }
}

/// A setup that could not start, for `reason`.
fn failed_before_run(cx: &Cx, reason: String) -> SetupRun {
    cx.warn(Warning {
        code: "setup_failed".to_owned(),
        message: format!("setup did not run: {reason}"),
    });
    SetupRun::Failed {
        line: reason,
        exit: None,
        ms: 0,
        tail: String::new(),
    }
}

/// Names the other repo in a setup line event, so its steps show which tree they ran in.
pub(super) fn named(event: ExtraEvent, repo: Option<&ForgeRepo>) -> ExtraEvent {
    match (event, repo) {
        (ExtraEvent::SetupLineStarted { index, total, line }, Some(repo)) => {
            ExtraEvent::SetupLineStarted {
                index,
                total,
                line: format!("{repo}: {line}"),
            }
        }
        (event, _) => event,
    }
}
