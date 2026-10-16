//! Time-budget checks for the real-Box end-to-end run.
//!
//! Pure logic only: events recorded by moor go in, per-phase timings and a
//! pass/fail verdict come out.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// One persisted moor event: monotonically increasing id, snake_case kind, unix ms.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Event {
    /// Insertion order; later events have larger ids.
    pub id: i64,
    /// Persisted snake_case kind name, e.g. `box_ready`.
    pub kind: String,
    /// Unix timestamp in milliseconds.
    pub ts_ms: i64,
}

/// A timed phase and the budget it must fit in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    /// Dotted phase name, e.g. `start.box_ready`.
    pub name: String,
    /// Measured duration; `None` when an endpoint event never happened.
    pub took_ms: Option<i64>,
    /// Maximum allowed duration, inclusive; `None` for an informational phase.
    pub budget_ms: Option<i64>,
}

fn secs(ms: i64) -> String {
    format!("{:.1}", ms as f64 / 1000.0)
}

impl Phase {
    /// True when the phase was measured and took no longer than its budget.
    pub fn pass(&self) -> bool {
        self.took_ms
            .is_some_and(|t| self.budget_ms.is_none_or(|b| t <= b))
    }

    /// One report line, e.g. `PHASE start.box_ready took=12.3s budget=30.0s PASS`.
    pub fn line(&self) -> String {
        let took = self
            .took_ms
            .map_or_else(|| "missing".to_string(), |t| format!("{}s", secs(t)));
        let word = if self.pass() { "PASS" } else { "FAIL" };
        let budget = self
            .budget_ms
            .map_or_else(|| "none".to_string(), |b| format!("{}s", secs(b)));
        format!("PHASE {} took={took} budget={budget} {word}", self.name)
    }
}

/// Parses one JSON [`Event`] per non-empty line; errors name the 1-based line.
pub fn parse_jsonl(text: &str) -> Result<Vec<Event>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("line {}: {e}", i + 1)))
        .collect()
}

/// Budgets for the start flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartBudgets {
    /// `started` to `box_ready`.
    pub box_ready_ms: i64,
    /// `setup_finished` (else `box_ready`) to `proxy_started`.
    pub proxy_ms: i64,
    /// `proxy_started` to `session_started`.
    pub session_ms: i64,
    /// `started` to `session_started`, minus the setup duration.
    pub total_ms: i64,
}

impl Default for StartBudgets {
    fn default() -> Self {
        Self {
            box_ready_ms: 30_000,
            proxy_ms: 15_000,
            session_ms: 20_000,
            total_ms: 90_000,
        }
    }
}

/// Earliest event of `kind`, optionally only those with an id above `after`.
fn first<'a>(events: &'a [Event], kind: &str, after: Option<&Event>) -> Option<&'a Event> {
    events
        .iter()
        .filter(|e| e.kind == kind && after.is_none_or(|a| e.id > a.id))
        .min_by_key(|e| e.id)
}

/// Like [`first`], but requires the anchor to exist.
fn next<'a>(events: &'a [Event], kind: &str, anchor: Option<&Event>) -> Option<&'a Event> {
    anchor.and_then(|a| first(events, kind, Some(a)))
}

fn span(from: Option<i64>, to: Option<&Event>) -> Option<i64> {
    Some(to?.ts_ms - from?)
}

fn phase(name: String, took_ms: Option<i64>, budget_ms: i64) -> Phase {
    Phase {
        name,
        took_ms,
        budget_ms: Some(budget_ms),
    }
}

/// Builds the start-flow phases in report order.
///
/// `start.setup` (the pilot's own `setup:` workload, `box_ready` to
/// `setup_finished`) is informational and is excluded from `start.total`.
pub fn start_phases(events: &[Event], b: &StartBudgets) -> Vec<Phase> {
    let started = first(events, "started", None);
    let box_ready = next(events, "box_ready", started);
    let setup_finished = next(events, "setup_finished", box_ready);
    let proxy_from = setup_finished.or(box_ready);
    let proxy = next(events, "proxy_started", proxy_from);
    let session_from_proxy = first(events, "proxy_started", None);
    let session = next(events, "session_started", session_from_proxy);
    let t0 = started.map(|e| e.ts_ms);
    let session_after_start = next(events, "session_started", started);
    let setup_ms = setup_finished.and_then(|f| span(box_ready.map(|e| e.ts_ms), Some(f)));
    let mut phases = vec![phase(
        "start.box_ready".into(),
        span(t0, box_ready),
        b.box_ready_ms,
    )];
    if setup_ms.is_some() {
        phases.push(Phase {
            name: "start.setup".into(),
            took_ms: setup_ms,
            budget_ms: None,
        });
    }
    phases.extend([
        phase(
            "start.proxy".into(),
            span(proxy_from.map(|e| e.ts_ms), proxy),
            b.proxy_ms,
        ),
        phase(
            "start.session".into(),
            span(session_from_proxy.map(|e| e.ts_ms), session),
            b.session_ms,
        ),
        phase(
            "start.total".into(),
            span(t0, session_after_start).map(|t| t - setup_ms.unwrap_or(0)),
            b.total_ms,
        ),
    ]);
    phases
}

/// Budgets for one sleep/wake cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeBudgets {
    /// Wake command to `woke`.
    pub woke_ms: i64,
    /// `woke` to `relaunched`.
    pub relaunch_ms: i64,
    /// Wake command to `relaunched`.
    pub total_ms: i64,
}

impl Default for WakeBudgets {
    fn default() -> Self {
        Self {
            woke_ms: 20_000,
            relaunch_ms: 25_000,
            total_ms: 45_000,
        }
    }
}

/// Builds the wake phases for `cycle`; `events` must be only those recorded
/// after the wake command began at `cmd_ms`.
pub fn wake_phases(events: &[Event], cmd_ms: i64, cycle: u32, b: &WakeBudgets) -> Vec<Phase> {
    let woke = first(events, "woke", None);
    let relaunched = next(events, "relaunched", woke);
    vec![
        phase(
            format!("wake{cycle}.woke"),
            span(Some(cmd_ms), woke),
            b.woke_ms,
        ),
        phase(
            format!("wake{cycle}.relaunch"),
            span(woke.map(|e| e.ts_ms), relaunched),
            b.relaunch_ms,
        ),
        phase(
            format!("wake{cycle}.total"),
            span(Some(cmd_ms), relaunched),
            b.total_ms,
        ),
    ]
}

/// A phase timed outside the event log (e.g. by wall clock).
pub fn timed(name: &str, took_ms: i64, budget_ms: i64) -> Phase {
    phase(name.to_string(), Some(took_ms), budget_ms)
}

/// Number of events whose kind equals `kind`.
pub fn count_kind(events: &[Event], kind: &str) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

/// True when the proxy log has a successful `POST <route>` line stamped at or after `after_ms`.
///
/// Lines are `<unix-ms> <method> <route> <status>`; nothing else in a line is read.
pub fn model_request_after(log: &str, after_ms: i64, route: &str) -> bool {
    log.lines().any(|line| {
        let mut f = line.split_whitespace();
        let (Some(ts), Some(method), Some(r), Some(status), None) =
            (f.next(), f.next(), f.next(), f.next(), f.next())
        else {
            return false;
        };
        method == "POST"
            && r == route
            && status.starts_with('2')
            && ts.parse::<i64>().is_ok_and(|t| t >= after_ms)
    })
}

/// True when the list is non-empty and every phase passes.
pub fn verdict(phases: &[Phase]) -> bool {
    !phases.is_empty() && phases.iter().all(Phase::pass)
}

/// How one report line ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Measured and within budget.
    Pass,
    /// Failed, over budget or missing.
    Fail,
    /// Could not be attempted.
    Skip,
}

/// Splits a report line into its phase name and outcome, if it is one.
pub fn parse_phase_line(line: &str) -> Option<(&str, Outcome)> {
    let mut tokens = line.split_whitespace();
    if tokens.next()? != "PHASE" {
        return None;
    }
    let name = tokens.next()?;
    let outcome = tokens.find_map(|t| match t {
        "PASS" => Some(Outcome::Pass),
        "FAIL" => Some(Outcome::Fail),
        "SKIP" => Some(Outcome::Skip),
        _ => None,
    })?;
    Some((name, outcome))
}

/// Report line for a phase that is timed but has no budget.
pub fn report_line(name: &str, took_ms: i64) -> String {
    format!("PHASE {name} took={}s budget=none PASS", secs(took_ms))
}

/// Verdict over report lines: the exit code (0 pass, 1 fail, 3 incomplete) and the summary line.
/// A run with any skipped phase is never a pass.
pub fn summarize(phases: &[(&str, Outcome)]) -> (u8, String) {
    let names =
        |o: Outcome| -> Vec<&str> { phases.iter().filter(|p| p.1 == o).map(|p| p.0).collect() };
    let (failed, skipped) = (names(Outcome::Fail), names(Outcome::Skip));
    if phases.is_empty() {
        return (1, "VERDICT FAIL (no phases)".to_owned());
    }
    let skips = if skipped.is_empty() {
        String::new()
    } else {
        format!("{} skipped: {}", skipped.len(), skipped.join(", "))
    };
    if !failed.is_empty() {
        let sep = if skips.is_empty() { "" } else { "; " };
        let line = format!(
            "VERDICT FAIL ({} of {} phases failed: {}{sep}{skips})",
            failed.len(),
            phases.len(),
            failed.join(", ")
        );
        return (1, line);
    }
    if skipped.is_empty() {
        return (0, format!("VERDICT PASS ({} phases)", phases.len()));
    }
    (
        3,
        format!("VERDICT INCOMPLETE ({} phases, {skips})", phases.len()),
    )
}

/// Final summary line naming any failed phases.
pub fn verdict_line(phases: &[Phase]) -> String {
    let parsed: Vec<(&str, Outcome)> = phases
        .iter()
        .map(|p| {
            let o = if p.pass() {
                Outcome::Pass
            } else {
                Outcome::Fail
            };
            (p.name.as_str(), o)
        })
        .collect();
    summarize(&parsed).1
}
