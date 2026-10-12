//! Box setup script.

use std::fmt::Write as _;

use super::quote;

#[cfg(test)]
mod tests;

/// Runs before the steps: the subshell logs which step failed when it exits non-zero.
const PRELUDE: &str = "trap '__moor_e=$?; [ \"$__moor_e\" -eq 0 ] || echo \"moor setup: step $__moor_step exited $__moor_e\"' EXIT\n";

/// POSIX sh text that runs `steps` in order, each in its own `sh -c`, and stops at the first
/// non-zero step. Steps share no shell state, so none can end, skip or unbalance the script.
///
/// Output of every step goes to stdout and `.moor/setup.log`; the exit code goes to
/// `.moor/setup.rc` and is the script's own exit code. Each step is announced with
/// `moor setup: $ <step>` (printed, never evaluated). No steps still writes rc `0`.
#[must_use]
pub fn setup_script(steps: &[String]) -> String {
    let mut body = String::new();
    for (i, step) in steps.iter().enumerate() {
        let announce = quote(&format!("moor setup: $ {step}"));
        let run = quote(step);
        let _ = writeln!(body, "__moor_step={}", i + 1);
        let _ = writeln!(body, "printf '%s\\n' {announce}\nsh -c {run} || exit $?");
    }
    format!(
        "mkdir -p .moor\n{{ (\n{PRELUDE}{body}) 2>&1; echo $? > .moor/setup.rc; }} | tee .moor/setup.log\n__moor_rc=$(cat .moor/setup.rc)\nexit \"${{__moor_rc:-1}}\"\n"
    )
}
