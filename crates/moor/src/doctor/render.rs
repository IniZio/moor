//! The report as aligned text or JSON.

use moor_core::Response;

use super::{Check, Status};
use crate::cli::{self, GlobalOpts, Output};

/// Width of the status column, wide enough for `warn` and `fail`.
const STATUS_WIDTH: usize = 5;
/// Width of the check column, wide enough for `controller`.
const NAME_WIDTH: usize = 11;

/// The report for `checks`, with exit code 1 when any check failed.
pub(super) fn output(checks: &[Check], opts: &GlobalOpts) -> Output {
    let code = u8::from(checks.iter().any(|c| c.status == Status::Fail));
    if opts.json {
        let value = serde_json::to_value(checks).ok();
        let out = cli::render(&Response::ok("doctor", value), opts);
        return Output { code, ..out };
    }
    Output {
        stdout: text(checks),
        stderr: String::new(),
        code,
    }
}

/// One line per check.
fn text(checks: &[Check]) -> String {
    checks.iter().map(line).collect()
}

/// `status  check  detail; fix: ...` and a line break.
fn line(c: &Check) -> String {
    let status = match c.status {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "fail",
    };
    let fix = if c.fix.is_empty() {
        String::new()
    } else {
        format!("; fix: {}", c.fix)
    };
    format!(
        "{status:<STATUS_WIDTH$} {:<NAME_WIDTH$} {}{fix}\n",
        c.name, c.detail
    )
}
