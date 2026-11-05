//! Attaches to the detached run that holds a setup script in the Box, launching it once.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use moor_core::boxscript::detached::attached_script;
use moor_core::boxscript::quote;
use moor_core::boxscript::setup::setup_script;
use moor_core::config::BoxSettings;
use moor_core::domain::decision::ExtraEvent;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::BoxPath;

use super::lines::{self, follow_lines};
use super::{private_dir, Failure};
use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};

/// Attaches after a cut channel before the setup is given up; the run in the Box goes on.
const REFOLLOWS: u32 = 5;

/// Pause before attaching again after a cut.
const REFOLLOW_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

/// Seconds a stopped setup run gets between TERM and KILL.
const CANCEL_GRACE_SECS: u32 = 5;

/// The id of the detached run that holds one setup script in the Box: the same for every
/// attach to one start's script, so a restarted controller follows the run it left. `attempt`
/// names the start, `workdir` the tree and `place` the line of a script run line by line.
pub(crate) fn run_id(attempt: i64, workdir: &BoxPath, place: Option<(u32, u32)>) -> i64 {
    use sha2::{Digest as _, Sha256};
    let key = format!("{attempt}\n{}\n{place:?}", workdir.as_str());
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(digest.get(..8).unwrap_or(&[0; 8]));
    // Negative, so it never names a detached command run, whose ids count up from 1.
    -i64::try_from(u64::from_le_bytes(bytes) >> 2).unwrap_or(0) - 1
}

/// Builds the setup environment, then attaches to run `run`, which launches the script once,
/// until the run ends, its output into `output`. A cut channel attaches again and reads the
/// log from its start, so only lines not reported yet reach `on_line`. A run given up on while
/// it may still be going is stopped in the Box.
pub(super) fn attach(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    (run, output): (i64, &Path),
    on_line: &(dyn Fn(ExtraEvent) + Sync),
) -> Result<ExecOutcome, Failure> {
    let purpose = Purpose::Setup {
        lines: cfg.setup.len(),
    };
    let exec = box_exec::prepare(cx, site, proxy, cfg, purpose).map_err(Failure::Prepare)?;
    if let Some(dir) = output.parent() {
        private_dir(dir).map_err(|e| Failure::Prepare(format!("{}: {e}", dir.display())))?;
    }
    let script = ["sh", "-c", &setup_script(&cfg.setup)].map(quote).join(" ");
    let spec = ExecSpec {
        argv: exec.argv(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            attached_script(run, &script),
        ]),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: Some(Output::Log(output.to_path_buf())),
    };
    let Deadline(limit) = exec.deadline();
    let started = cx.clock().instant();
    let reported = Reported::default();
    let deliver = |e: ExtraEvent| {
        if reported.first(&e) {
            on_line(e);
        }
    };
    let mut cuts = 0;
    loop {
        // A log left by an earlier attach would announce lines this one has not reached.
        let _ = std::fs::remove_file(output);
        let left = limit.saturating_sub(cx.clock().instant().saturating_duration_since(started));
        let result = follow_lines(cx, cfg, output, &deliver, || {
            cx.driver()
                .exec(site.handle, &spec, Deadline(left))
                .map_err(Failure::Exec)
        });
        let ended = matches!(
            result,
            Ok(ExecOutcome::Exited { .. }) | Err(Failure::Prepare(_))
        );
        let more = lines::cut(&result) && cuts < REFOLLOWS && !left.is_zero();
        if ended || !more || cx.env().cancel().is_cancelled() {
            if !ended {
                let _ = cx.driver().cancel_run(site.handle, run, CANCEL_GRACE_SECS);
            }
            return result;
        }
        cuts += 1;
        cx.clock().sleep(REFOLLOW_AFTER);
    }
}

/// The highest line each kind of line event has reported, so an attach that reads the log from
/// its start again reports only what is new.
#[derive(Default)]
struct Reported {
    /// Highest line reported started.
    started: AtomicU32,
    /// Highest line reported finished.
    finished: AtomicU32,
}

impl Reported {
    /// Whether `event` is reported for the first time; records it.
    fn first(&self, event: &ExtraEvent) -> bool {
        let (seen, index) = match event {
            ExtraEvent::SetupLineStarted { index, .. } => (&self.started, *index),
            ExtraEvent::SetupLineFinished { index, .. } => (&self.finished, *index),
            _ => return true,
        };
        seen.fetch_max(index, Ordering::SeqCst) < index
    }
}
