//! Copies the agent's Reports out of a Box into the local store before teardown.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, ExecSpec, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, Warning};

use super::provision::recorded_workdir;
use crate::cx::Cx;
use crate::store::{DriverRow, Store, TaskRow};

/// Wall-clock allowance for the index script.
pub(crate) const INDEX_DEADLINE: Duration = Duration::from_secs(60);
/// Largest Report, in bytes.
const MAX_BYTES: u64 = 256 * 1024;
/// Most Reports taken from one Box.
const MAX_FILES: usize = 50;
/// Index file, relative to the workdir.
const INDEX_FILE: &str = ".moor/reports.index";
/// Reports directory, relative to the workdir.
const REPORTS_DIR: &str = ".moor/reports";

/// Lists regular, non-symlink `*.md` files as `<name>\t<size>`; the index is truncated first.
const INDEX_SCRIPT: &str = "mkdir -p .moor && : > .moor/reports.index && \
for f in .moor/reports/*.md; do \
[ -f \"$f\" ] && [ ! -L \"$f\" ] || continue; \
printf '%s\\t%s\\n' \"${f##*/}\" \"$(wc -c < \"$f\" | tr -d ' ')\" >> .moor/reports.index; \
done";

/// One line of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// File name, without the directory.
    pub(crate) name: String,
    /// Size in bytes.
    pub(crate) size: u64,
}

/// The kind of a Report file: its name minus one `.md`, when the name is
/// `[A-Za-z0-9][A-Za-z0-9._-]*\.md`.
pub(crate) fn kind_of(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".md")?;
    let mut bytes = stem.bytes();
    let first = bytes.next()?;
    (first.is_ascii_alphanumeric()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')))
    .then_some(stem)
}

/// Whether `name` is a valid Report file name.
pub(crate) fn valid_name(name: &str) -> bool {
    kind_of(name).is_some()
}

/// Parses the index into entries sorted by name, and counts the malformed lines.
pub(crate) fn parse_index(text: &str) -> (Vec<Entry>, usize) {
    let mut bad = 0;
    let mut entries = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        match line.split_once('\t') {
            Some((name, size)) => match size.trim().parse::<u64>() {
                Ok(size) => entries.push(Entry {
                    name: name.to_owned(),
                    size,
                }),
                Err(_) => bad += 1,
            },
            None => bad += 1,
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    (entries, bad)
}

/// The Ready Box with a handle that Reports are collected from.
pub(super) fn target(row: &TaskRow) -> Option<(&DriverRow, &BoxHandle)> {
    row.boxed
        .as_ref()
        .filter(|b| b.state == BoxState::Ready)
        .and_then(|b| b.handle.as_ref().map(|h| (b, h)))
}

/// The host directory the driver runs from.
pub(super) fn host_of(row: &TaskRow) -> &Path {
    Path::new(row.worktree.as_deref().unwrap_or(&row.repo))
}

/// The steps that index a Box's Reports, to run in a batch with other steps.
pub(crate) struct IndexPlan {
    /// The Box workdir the Reports live under.
    workdir: BoxPath,
    /// Runs the index script in the workdir, then reads the index.
    pub(crate) steps: Vec<BoxStep>,
}

/// Plans the index steps, or `None` when nothing is collected from this Box or the workdir is
/// unknown; [`collect_from`] then collects, and warns, exactly as [`collect`] does.
pub(crate) fn index_plan(cx: &Cx, row: &TaskRow) -> Option<IndexPlan> {
    let (b, handle) = target(row)?;
    let workdir = match b.workdir.as_deref().and_then(recorded_workdir) {
        Some(w) => w,
        None => cx.driver().workdir(handle, host_of(row)).ok()?,
    };
    let index = BoxPath::under(workdir.as_str(), INDEX_FILE).ok()?;
    // A batched `Run` has no working directory, so the script changes into the workdir itself.
    let script = format!("cd {} && {INDEX_SCRIPT}", quote(workdir.as_str()));
    Some(IndexPlan {
        workdir,
        steps: vec![BoxStep::Run(script), BoxStep::Peek(index)],
    })
}

/// Copies new Report versions from a Ready Box into the store; returns how many were stored.
///
/// A failure only warns, so the close outcome never changes. Report contents never appear in
/// a warning.
pub(crate) fn collect(cx: &Cx, store: &Store, row: &TaskRow) -> usize {
    let Some((b, handle)) = target(row) else {
        return 0;
    };
    let mut stored = 0;
    if let Err(reason) = run(cx, store, row, b.workdir.as_deref(), handle, &mut stored) {
        failed(cx, &reason);
    }
    stored
}

/// Stores the Reports from the results of an [`IndexPlan`]'s steps. When the batch stopped
/// before them, it collects as [`collect`] does.
pub(crate) fn collect_from(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    plan: &IndexPlan,
    done: &mut dyn Iterator<Item = Result<StepDone, DriverError>>,
) -> usize {
    let (Some((_, handle)), Some(ran)) = (target(row), done.next()) else {
        return collect(cx, store, row);
    };
    let mut stored = 0;
    let result = indexed(ran, done.next()).and_then(|index| {
        read_reports(cx, store, row, (handle, &plan.workdir), index, &mut stored)
    });
    if let Err(reason) = result {
        failed(cx, &reason);
    }
    stored
}

/// Indexes the Box's Reports and stores each accepted one.
fn run(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    recorded: Option<&str>,
    handle: &BoxHandle,
    stored: &mut usize,
) -> Result<(), String> {
    let host = host_of(row);
    let workdir = match recorded.and_then(recorded_workdir) {
        Some(w) => w,
        None => cx
            .driver()
            .workdir(handle, host)
            .map_err(|e| format!("cannot find the box workdir: {e}"))?,
    };
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), INDEX_SCRIPT.to_owned()],
        env: Vec::new(),
        cwd: Some(workdir.clone()),
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(host.to_path_buf()),
        class: None,
    };
    match cx.driver().exec(handle, &spec, Deadline(INDEX_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        Ok(ExecOutcome::Exited { code }) => return Err(format!("the index script exited {code}")),
        Ok(ExecOutcome::Interrupted) => return Err("the index script was interrupted".into()),
        Err(e) => return Err(format!("the index script could not run: {e}")),
    }
    let index = peek(cx, handle, &workdir, INDEX_FILE, host)?;
    read_reports(cx, store, row, (handle, &workdir), index, stored)
}

/// The index text from the batched index script and index read; `None` when the file is absent.
fn indexed(
    ran: Result<StepDone, DriverError>,
    peeked: Option<Result<StepDone, DriverError>>,
) -> Result<Option<String>, String> {
    match ran {
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            ..
        }) => {}
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code },
            ..
        }) => return Err(format!("the index script exited {code}")),
        Ok(StepDone::Ran {
            outcome: ExecOutcome::Interrupted,
            ..
        }) => return Err("the index script was interrupted".into()),
        Ok(_) => return Err("the index script returned no result".into()),
        Err(e) => return Err(format!("the index script could not run: {e}")),
    }
    match peeked {
        Some(Ok(StepDone::Peeked(text))) => Ok(text),
        Some(Err(e)) => Err(format!("cannot read a report file: {e}")),
        _ => Err("cannot read a report file: the index read returned no result".into()),
    }
}

/// Stores each accepted Report listed in `index`.
fn read_reports(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    (handle, workdir): (&BoxHandle, &BoxPath),
    index: Option<String>,
    stored: &mut usize,
) -> Result<(), String> {
    let host = host_of(row);
    let Some(index) = index else {
        return Ok(());
    };
    let (entries, bad) = parse_index(&index);
    if bad > 0 {
        skipped(cx, "an index line was unreadable and was skipped");
    }
    let dir = cx
        .state_dir()
        .map_err(|e| e.error.to_string())?
        .join("reports");
    for entry in accepted(cx, entries) {
        let rel = format!("{REPORTS_DIR}/{}", entry.name);
        let Some(kind) = kind_of(&entry.name) else {
            continue;
        };
        let text = match peek(cx, handle, workdir, &rel, host) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(_) => {
                skipped(cx, "a report could not be read and was skipped");
                continue;
            }
        };
        if text.len() as u64 > MAX_BYTES {
            skipped(cx, &too_big(&entry.name));
            continue;
        }
        match store.add_report(row.id, kind, &text, &dir) {
            Ok(Some(_)) => *stored += 1,
            Ok(None) => {}
            Err(_) => skipped(cx, "a report could not be stored and was skipped"),
        }
    }
    Ok(())
}

/// Reads a file under the workdir; an error carries no file contents.
fn peek(
    cx: &Cx,
    handle: &BoxHandle,
    workdir: &BoxPath,
    rel: &str,
    host: &Path,
) -> Result<Option<String>, String> {
    let path = BoxPath::under(workdir.as_str(), rel).map_err(|e| format!("bad path: {e}"))?;
    cx.driver()
        .peek_file(handle, &path, host)
        .map_err(|e| format!("cannot read a report file: {e}"))
}

/// Drops entries with unsafe names, oversize files and any beyond the file cap, warning for each.
fn accepted(cx: &Cx, entries: Vec<Entry>) -> Vec<Entry> {
    let mut kept = Vec::new();
    let mut over = false;
    for entry in entries {
        if !valid_name(&entry.name) {
            skipped(cx, "a report with an unsupported file name was skipped");
        } else if entry.size > MAX_BYTES {
            skipped(cx, &too_big(&entry.name));
        } else if kept.len() >= MAX_FILES {
            over = true;
        } else {
            kept.push(entry);
        }
    }
    if over {
        skipped(cx, &format!("only the first {MAX_FILES} reports are kept"));
    }
    kept
}

/// Message for a Report over the size cap.
fn too_big(name: &str) -> String {
    format!(
        "report {name} is over the {} KiB cap and was skipped",
        MAX_BYTES / 1024
    )
}

/// Warns that one Report was left out.
fn skipped(cx: &Cx, message: &str) {
    cx.warn(Warning {
        code: "report_skipped".to_owned(),
        message: message.to_owned(),
    });
}

/// Warns that collecting stopped; the close goes on.
fn failed(cx: &Cx, reason: &str) {
    cx.warn(Warning {
        code: "reports_collect_failed".to_owned(),
        message: format!("reports were not fully saved: {reason}; the close goes on"),
    });
}

#[cfg(test)]
mod tests;
