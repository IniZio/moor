//! Tells which `setup:` line is running from the script's own announcements.

use std::path::Path;
use std::time::Instant;

use moor_core::config::BoxSettings;
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::seam::proc::Output;

use super::Failure;
use crate::cx::Cx;
use crate::ops::run::follow;

/// What the script prints before it runs each line.
pub(super) const ANNOUNCE: &str = "moor setup: $ ";

/// Longest line text an event carries, in characters.
const LABEL_CHARS: usize = 80;

/// Follows the setup script's output and reports each line's start and end.
///
/// The script stops at the first failing line, so a line that is followed by another one passed,
/// and the last one ends with the script.
pub(super) struct Lines<'a> {
    /// The repo's setup lines.
    steps: &'a [String],
    /// Output of the line being read, held until its newline arrives.
    pending: String,
    /// The announced line that has not ended: its position from 1 and when it began.
    open: Option<(u32, Instant)>,
    /// How many lines the script has announced.
    announced: u32,
}

impl<'a> Lines<'a> {
    /// Follows the script that runs `steps`.
    pub(super) const fn new(steps: &'a [String]) -> Self {
        Self {
            steps,
            pending: String::new(),
            open: None,
            announced: 0,
        }
    }

    /// Takes the next piece of the script's output, read at `now`.
    pub(super) fn feed(&mut self, bytes: &[u8], now: Instant) -> Vec<ExtraEvent> {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        while let Some(end) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=end).collect();
            if line.starts_with(ANNOUNCE) {
                events.extend(self.announce(now));
            }
        }
        events
    }

    /// Ends the line still running, when the script has ended with `ok`.
    pub(super) fn finish(&mut self, ok: bool, now: Instant) -> Vec<ExtraEvent> {
        self.close(ok, now).into_iter().collect()
    }

    /// Handles the script announcing its next line.
    fn announce(&mut self, now: Instant) -> Vec<ExtraEvent> {
        let mut events: Vec<ExtraEvent> = self.close(true, now).into_iter().collect();
        self.announced += 1;
        let index = self.announced;
        let Some(step) = usize::try_from(index)
            .ok()
            .and_then(|i| self.steps.get(i - 1))
        else {
            return events;
        };
        self.open = Some((index, now));
        events.push(ExtraEvent::SetupLineStarted {
            index,
            total: self.total(),
            line: label(step),
        });
        events
    }

    /// Ends the open line, if any, with `ok`.
    fn close(&mut self, ok: bool, now: Instant) -> Option<ExtraEvent> {
        let (index, began) = self.open.take()?;
        let ms =
            u64::try_from(now.saturating_duration_since(began).as_millis()).unwrap_or(u64::MAX);
        Some(ExtraEvent::SetupLineFinished {
            index,
            total: self.total(),
            ms,
            ok,
        })
    }

    /// How many lines the repo declares.
    fn total(&self) -> u32 {
        u32::try_from(self.steps.len()).unwrap_or(u32::MAX)
    }
}

/// `step` cut to its first line and [`LABEL_CHARS`] characters, marked with `…` when cut.
fn label(step: &str) -> String {
    let step = step.trim();
    let first = step.lines().next().unwrap_or_default();
    let mut out: String = first.chars().take(LABEL_CHARS).collect();
    if out.chars().count() < first.chars().count() || first.len() < step.len() {
        out.push('…');
    }
    out
}

/// Runs `exec`, telling `on_line` when each setup line starts and ends as the script announces them.
pub(super) fn follow_lines(
    cx: &Cx,
    cfg: &BoxSettings,
    output: &Path,
    on_line: &(dyn Fn(ExtraEvent) + Sync),
    exec: impl FnOnce() -> Result<ExecOutcome, Failure>,
) -> Result<ExecOutcome, Failure> {
    let mut lines = Lines::new(&cfg.setup);
    let clock = cx.clock();
    let result = follow::during(
        Some(&Output::Log(output.to_path_buf())),
        Some(&mut |bytes: &[u8]| {
            lines
                .feed(bytes, clock.instant())
                .into_iter()
                .for_each(on_line);
            Ok(())
        }),
        exec,
    );
    if !cut(&result) {
        let ok = matches!(result, Ok(ExecOutcome::Exited { code: 0 }));
        lines
            .finish(ok, clock.instant())
            .into_iter()
            .for_each(on_line);
    }
    result
}

/// Whether `result` is a channel that broke before the script ended and its deadline passed.
pub(super) fn cut(result: &Result<ExecOutcome, Failure>) -> bool {
    match result {
        Ok(ExecOutcome::Interrupted) => true,
        Err(Failure::Exec(e)) => !matches!(e, DriverError::Timeout(_)),
        _ => false,
    }
}
