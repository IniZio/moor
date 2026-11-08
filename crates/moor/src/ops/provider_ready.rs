//! `provider-ready`: whether each node runtime's provider credentials are in place here.
//!
//! Only the name, status, hint, error class and error text of a failed check leave the
//! controller: crabbox's messages and other details hold paths, key fingerprints and adapter
//! output. The class is kept only when it is a short token, the error only as one bounded line.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};
use moor_core::view::node::{ProviderCheck, ProviderReady, ProviderReadyValue};
use serde_json::Value;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::nodes::all;

/// Ceiling for one `crabbox doctor`.
const DEADLINE_SECS: u64 = 60;

/// Longest error class kept.
const MAX_CLASS_LEN: usize = 32;

/// Longest error text kept, in characters.
const MAX_ERROR_LEN: usize = 200;

/// One entry per distinct runtime of the controller's nodes, sorted.
///
/// # Errors
/// A store failure.
pub(crate) fn run(cx: &Cx) -> Result<ProviderReadyValue, CommandError> {
    let runtimes: BTreeSet<String> = all(cx)?.into_iter().map(|n| n.runtime).collect();
    let providers = runtimes
        .into_iter()
        .map(|provider| {
            let failing = doctor(cx, &provider);
            ProviderReady {
                provider,
                ok: failing.is_empty(),
                failing,
            }
        })
        .collect();
    Ok(ProviderReadyValue { providers })
}

/// The failed checks of `crabbox doctor` for `provider`.
fn doctor(cx: &Cx, provider: &str) -> Vec<ProviderCheck> {
    let Some(dir) = scratch_dir(cx) else {
        return vec![unrunnable()];
    };
    let spec = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(DEADLINE_SECS)))
        .args(["doctor", "--provider", provider, "--json"])
        .cwd(dir);
    let stdout = match cx.proc().run(&spec) {
        Ok(out) => out.stdout,
        // crabbox exits non-zero when a check fails, and still prints the report.
        Err(ProcError::NonZero { stdout, .. }) => stdout,
        Err(_) => return vec![unrunnable()],
    };
    parse(&stdout).unwrap_or_else(|| vec![unrunnable()])
}

/// An empty directory moor owns, so crabbox never reads the controller's working directory as
/// repository config.
fn scratch_dir(cx: &Cx) -> Option<PathBuf> {
    let dir = cx.state_dir().ok()?.join("provider-ready");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The class when it is a short token, never free text.
fn class_of(details: Option<&Value>) -> Option<String> {
    let class = details?.get("class")?.as_str()?;
    let token = !class.is_empty()
        && class.len() <= MAX_CLASS_LEN
        && class
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    token.then(|| class.to_owned())
}

/// The error text as one line without control characters, cut to [`MAX_ERROR_LEN`].
fn error_of(details: Option<&Value>) -> Option<String> {
    let raw = details?.get("error")?.as_str()?;
    let line: String = raw
        .chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= MAX_ERROR_LEN {
        return Some(line.to_owned());
    }
    let mut cut: String = line.chars().take(MAX_ERROR_LEN - 1).collect();
    cut.push('…');
    Some(cut)
}

/// The check for a crabbox that gave no report.
fn unrunnable() -> ProviderCheck {
    ProviderCheck {
        check: "crabbox".to_owned(),
        status: "failed".to_owned(),
        class: None,
        error: None,
        hint: None,
    }
}

/// The failed checks in the last stdout line that is a JSON object; adapters may print before it.
fn parse(stdout: &str) -> Option<Vec<ProviderCheck>> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))?;
    let report: Value = serde_json::from_str(line).ok()?;
    let checks = report.get("checks")?.as_array()?;
    Some(
        checks
            .iter()
            .filter(|c| c.get("status").and_then(Value::as_str) == Some("failed"))
            .map(|c| ProviderCheck {
                check: c
                    .get("check")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                status: "failed".to_owned(),
                class: class_of(c.get("details")),
                error: error_of(c.get("details")),
                hint: c
                    .get("details")
                    .and_then(|d| d.get("hint"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests;
