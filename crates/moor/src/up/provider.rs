//! The crabbox provider `up` builds the local node on: the one crabbox selects, once its own
//! doctor passes.

use std::path::Path;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};
use serde_json::Value;

use crate::doctor::{Check, CRABBOX_INSTALL};

/// Ceiling for one `crabbox doctor`.
const DEADLINE_SECS: u64 = 60;

/// Commands that install a provider when crabbox selects none.
pub(crate) const INSTALL_PROVIDER: [&str; 2] = [
    "curl -fsSL https://github.com/IniZio/crabbox-cloudhypervisor/releases/download/v0.1.0/install.sh | sh",
    "crabbox-cloudhypervisor install",
];

/// What `crabbox doctor --json` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    /// The selected provider; empty when none is.
    pub provider: String,
    /// `check (hint)` for each failed check.
    pub failed: Vec<String>,
}

/// The selected provider once `crabbox doctor --provider` passes for it, else the failed check.
pub(crate) fn selected(proc: &dyn Proc, scratch: &Path) -> Result<(String, Check), Check> {
    let any = doctor(proc, scratch, &[])?;
    if any.provider.is_empty() {
        return Err(Check::fail(
            "provider",
            "crabbox selects no provider",
            "install one, then select it in the file `crabbox config path` prints, as the \
             provider's README shows; for cloud-hypervisor run:",
        ));
    }
    let provider = any.provider;
    let strict = doctor(proc, scratch, &["--provider", &provider])?;
    if !strict.failed.is_empty() {
        return Err(Check::fail(
            "provider",
            format!("{provider}: {}", strict.failed.join(", ")),
            format!("run `crabbox doctor --provider {provider}` and fix what it reports"),
        ));
    }
    let ok = Check::ok("provider", format!("{provider}, ready per crabbox doctor"));
    Ok((provider, ok))
}

/// Runs `crabbox doctor --json` with `args` in `scratch`.
fn doctor(proc: &dyn Proc, scratch: &Path, args: &[&str]) -> Result<Report, Check> {
    let spec = ProcSpec::new(Tool::Crabbox, Deadline(Duration::from_secs(DEADLINE_SECS)))
        .arg("doctor")
        .args(args.iter().copied())
        .arg("--json")
        .cwd(scratch.to_path_buf());
    let stdout = match proc.run(&spec) {
        Ok(out) => out.stdout,
        Err(ProcError::NonZero { stdout, .. }) if !stdout.trim().is_empty() => stdout,
        Err(ProcError::Spawn(_) | ProcError::NonZero { code: 127, .. }) => {
            return Err(Check::fail(
                "provider",
                "crabbox is not on PATH",
                CRABBOX_INSTALL,
            ));
        }
        Err(e) => {
            return Err(Check::fail(
                "provider",
                format!("crabbox doctor: {e}"),
                CRABBOX_INSTALL,
            ))
        }
    };
    parse(&stdout).ok_or_else(|| {
        Check::fail(
            "provider",
            "crabbox doctor --json printed no report",
            CRABBOX_INSTALL,
        )
    })
}

/// The report in the last stdout line that is a JSON object.
pub(crate) fn parse(stdout: &str) -> Option<Report> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))?;
    let report: Value = serde_json::from_str(line).ok()?;
    let provider = report.get("provider")?.as_str()?.to_owned();
    let failed = report
        .get("checks")?
        .as_array()?
        .iter()
        .filter(|c| c.get("status").and_then(Value::as_str) == Some("failed"))
        .map(|c| {
            let name = c.get("check").and_then(Value::as_str).unwrap_or("unknown");
            c.pointer("/details/hint")
                .and_then(Value::as_str)
                .map_or_else(|| name.to_owned(), |hint| format!("{name} ({hint})"))
        })
        .collect();
    Some(Report { provider, failed })
}
