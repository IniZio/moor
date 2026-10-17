//! The sandbox's account of what it denied during a failed setup.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::denials::{
    cap, from_report, parse, strip, window_script, Denial, SETUP_REPORT_REL,
};
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxPath};

use super::fetch_box_file;
use crate::cx::Cx;
use crate::ops::show::redact_nonce;

/// Most denials listed, newest last; the older ones are counted.
const MAX_DENIALS: usize = 10;

/// Most lines of the sandbox's own text shown when it names no denial.
const MAX_TEXT_LINES: usize = 5;

/// How long the Box may take to print the denial log.
const WINDOW_DEADLINE: Duration = Duration::from_secs(60);

/// Words the sandbox's denials for the failure warning, starting with a newline: the Seatbelt log
/// since setup started, plus nono's report; empty when they say nothing. Only a sandboxed exec is
/// asked about, so otherwise the Box is not asked.
pub(super) fn fetch(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    worktree: &Path,
    sandboxed: bool,
    nonce: Option<&[u8]>,
) -> String {
    if !sandboxed {
        return String::new();
    }
    // A report left by an earlier run must not be read as this one's.
    let _ = std::fs::remove_file(worktree.join(SETUP_REPORT_REL));
    let raw = fetch_box_file(cx, handle, workdir, worktree, SETUP_REPORT_REL);
    let mut found = window(cx, handle, workdir, worktree);
    let report = from_report(&raw);
    for d in report.denials {
        if !found.contains(&d) {
            found.push(d);
        }
    }
    describe(&found, &report.text, nonce)
}

/// The denials the Box's log holds since setup started; none when the Box cannot say.
fn window(cx: &Cx, handle: &BoxHandle, workdir: &BoxPath, worktree: &Path) -> Vec<Denial> {
    let Ok(state) = cx.state_dir() else {
        return Vec::new();
    };
    let log = state.join(format!("setup-denials-{}.log", std::process::id()));
    let spec = ExecSpec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            window_script(workdir.as_str()),
        ],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: Some(Output::Log(log.clone())),
        host_cwd: Some(worktree.to_path_buf()),
        class: None,
    };
    let ran = cx.driver().exec(handle, &spec, Deadline(WINDOW_DEADLINE));
    let text = std::fs::read_to_string(&log);
    let _ = std::fs::remove_file(&log);
    match (ran, text) {
        (Ok(ExecOutcome::Exited { code: 0 }), Ok(text)) => parse(&text),
        _ => Vec::new(),
    }
}

/// Words the denials, else nono's text, for the warning; each line is stripped, loses the nonce, then is capped.
pub(super) fn describe(denials: &[Denial], text: &str, nonce: Option<&[u8]>) -> String {
    let clean = |l: &str| cap(&redact_nonce(&strip(l), nonce));
    let mut out = String::new();
    if denials.is_empty() {
        let text: Vec<String> = text
            .lines()
            .map(clean)
            .filter(|l| !l.trim().is_empty())
            .take(MAX_TEXT_LINES)
            .collect();
        if !text.is_empty() {
            let _ = write!(out, "\nsandbox: {}", text.join("\n"));
        }
        return out;
    }
    out.push_str(
        "\nSeatbelt denials logged on the Box since setup started (may include other processes):",
    );
    let earlier = denials.len().saturating_sub(MAX_DENIALS);
    if earlier > 0 {
        let _ = write!(out, "\n  … and {earlier} earlier");
    }
    for d in denials.iter().skip(earlier) {
        let _ = write!(out, "\n  {}", clean(&d.to_string()));
    }
    out
}
