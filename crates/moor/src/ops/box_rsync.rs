//! `box-rsync`: the machine runner's rsync, run in a Box as `rsync --server` with its stdio
//! streamed over the connection. The runner mirrors a Box and puts files into it this way, so
//! it shares no disk with the controller.
//!
//! Holds the Box lock for the transfer, as `cp` does for its copy.

use std::path::{Component, Path};
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{BoxId, BoxRsyncReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::wake::wake_and_revive;
use crate::ops::cp::usable_handle;

/// Time allowed for one transfer.
pub(crate) const DEADLINE: Duration = Duration::from_mins(10);

/// The long options the runner's rsync passes its server. Any other could name a path the
/// check never sees, `--secluded-args` most of all, which sends the paths on stdin.
const LONG: [&str; 2] = ["--sender", "--mkpath"];

/// The letters of the server's short option cluster before its `e` capability string; none
/// takes an argument.
const SHORT: &str = "logDtprv";

/// Runs `req` in its Box with rsync's stdio on `pipes`, killing it once `within` has passed.
///
/// # Errors
/// [`CommandError`] when the argv is not the runner's rsync on the Box's work tree, the Box is
/// not open or has no machine, the transfer passes `within`, or the driver cannot stream.
pub(crate) fn run(
    cx: &Cx,
    req: &BoxRsyncReq,
    pipes: Pipes<'_>,
    within: Duration,
) -> Result<ExecView, CommandError> {
    let refused = |why: String| CommandError::new(ErrorKind::Refused, why);
    let (head, path) = server_argv(&req.argv).map_err(refused)?;
    let _held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        req.id,
        "box-rsync",
        boxlock::WAIT,
    )?;
    let (handle, workdir) = woken(cx, req.id)?;
    let mut argv = head;
    argv.push(beneath(&path, &workdir).map_err(refused)?);
    let spec = ExecSpec {
        argv,
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let clock = cx.clock();
    let end = clock.instant() + within;
    let late = || clock.instant() >= end;
    let Pipes {
        stdin,
        stdout,
        stderr,
        stop: gone,
    } = pipes;
    let stop = || gone() || late();
    let pipes = Pipes {
        stdin,
        stdout,
        stderr,
        stop: &stop,
    };
    let ran = cx.driver().exec_piped(&handle, &spec, pipes);
    let failed = |m: String| CommandError::new(ErrorKind::Failed, m);
    match ran {
        Ok(ExecOutcome::Interrupted) if late() => Err(failed(format!(
            "box-rsync ran past its {} minute deadline",
            within.as_secs() / 60
        ))),
        Ok(ExecOutcome::Exited { code }) => Ok(ExecView {
            exit: Some(code),
            timed_out: false,
        }),
        Ok(ExecOutcome::Interrupted) => Ok(ExecView {
            exit: None,
            timed_out: false,
        }),
        Err(e) => Err(failed(e.to_string())),
    }
}

/// The handle and work tree of open Box `id`, woken when it sleeps.
fn woken(cx: &Cx, id: BoxId) -> Result<(BoxHandle, BoxPath), CommandError> {
    let store = cx.store()?;
    let not_open = || {
        CommandError::new(
            ErrorKind::Failed,
            format!("no open Box with id {}", id.get()),
        )
    };
    let open = |row: &crate::store::BoxRow| {
        !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
    };
    let reload = || store.box_row(id)?.filter(open).ok_or_else(not_open);
    let mut row = reload()?;
    if wake_and_revive(cx, store, &row)? {
        row = reload()?;
    }
    let handle = usable_handle(&row)?;
    let recorded = row.boxed.as_ref().and_then(|b| b.workdir.as_deref());
    let workdir = match recorded.and_then(recorded_workdir) {
        Some(w) => w,
        None => cx.driver().workdir(&handle)?,
    };
    Ok((handle, workdir))
}

/// The words of `argv` up to its `.`, and the one path after it, unescaped; refused unless
/// it is `rsync --server` with only the options the runner's rsync sends.
fn server_argv(argv: &[String]) -> Result<(Vec<String>, String), String> {
    let not = || "box-rsync runs only `rsync --server [options] . PATH`".to_owned();
    let [rsync, server, rest @ ..] = argv else {
        return Err(not());
    };
    if rsync != "rsync" || server != "--server" {
        return Err(not());
    }
    let dot = rest.iter().position(|w| w == ".").ok_or_else(not)?;
    let (options, [_, path]) = rest.split_at(dot) else {
        return Err(not());
    };
    if let Some(bad) = options.iter().find(|o| !allowed(o)) {
        return Err(format!("box-rsync does not pass the rsync option {bad:?}"));
    }
    Ok((
        argv.get(..dot + 3).unwrap_or_default().to_vec(),
        unescape(path),
    ))
}

/// Whether `option` is one the runner's rsync sends its server.
fn allowed(option: &str) -> bool {
    if LONG.contains(&option) {
        return true;
    }
    let Some(cluster) = option.strip_prefix('-').filter(|c| !c.starts_with('-')) else {
        return false;
    };
    let (flags, caps) = cluster.split_once('e').unwrap_or((cluster, ""));
    flags.chars().all(|c| SHORT.contains(c))
        && caps.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// `word` as rsync's server reads it: rsync backslash-escapes the remote path for the shell
/// a remote shell would start, and the driver quotes each word itself.
fn unescape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    out
}

/// `path` in the Box's work tree: as given when absolute, else under `workdir`, its trailing
/// slash kept since rsync reads it; refused when it is outside or climbs with `..`.
fn beneath(path: &str, workdir: &BoxPath) -> Result<String, String> {
    let root = workdir.as_str().trim_end_matches('/');
    let full = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{root}/{path}")
    };
    let p = Path::new(&full);
    if p.components().any(|c| c == Component::ParentDir) || !p.starts_with(workdir.as_str()) {
        return Err(format!(
            "box-rsync takes only a path in the Box's work tree {root}, not {path:?}"
        ));
    }
    Ok(full)
}

#[cfg(test)]
mod tests;
