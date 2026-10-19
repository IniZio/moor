//! Box setup script.

use std::fmt::Write as _;

use super::quote;

#[cfg(test)]
mod tests;

/// Tools the Box scripts rely on. Setup warns, never fails, when one is missing.
pub const BOX_TOOLS: [&str; 5] = ["sh", "git", "curl", "tar", "unzip"];

/// POSIX sh text that runs `steps` in order, each in its own `sh -c`, and stops at the first
/// non-zero step. Steps share no shell state, so none can end, skip or unbalance the script.
///
/// Output of every step goes to stdout and `.moor/setup.log`; the exit code goes to
/// `.moor/setup.rc` and is the script's own exit code. Each step is announced with
/// `moor setup: $ <step>` (printed, never evaluated). No steps still writes rc `0`. A missing
/// tool from [`BOX_TOOLS`] is warned about first, in the log too, and never changes the rc.
#[must_use]
pub fn setup_script(steps: &[String]) -> String {
    let mut check = String::new();
    for tool in BOX_TOOLS {
        let warning = quote(&format!(
            "moor: warning: the Box has no {tool}; see the Box image requirements in the README"
        ));
        let _ = writeln!(
            check,
            "command -v {tool} >/dev/null 2>&1 || printf '%s\\n' {warning} >&2"
        );
    }
    build_script("setup", &check, steps)
}

/// The script behind [`setup_script`], with `label` naming its announcements and `.moor/<label>.*`
/// files.
pub(super) fn steps_script(label: &str, steps: &[String]) -> String {
    build_script(label, "", steps)
}

/// Shared body of the setup and teardown scripts; `check` runs before the first step.
fn build_script(label: &str, check: &str, steps: &[String]) -> String {
    // The subshell logs which step failed when it exits non-zero.
    let prelude = format!("trap '__moor_e=$?; [ \"$__moor_e\" -eq 0 ] || echo \"moor {label}: step $__moor_step exited $__moor_e\"' EXIT\n");
    let mut body = String::new();
    for (i, step) in steps.iter().enumerate() {
        let announce = quote(&format!("moor {label}: $ {step}"));
        let run = quote(step);
        let _ = writeln!(body, "__moor_step={}", i + 1);
        let _ = writeln!(body, "printf '%s\\n' {announce}\nsh -c {run} || exit $?");
    }
    format!(
        "mkdir -p .moor\n{{ (\n{prelude}{check}{body}) 2>&1; echo $? > .moor/{label}.rc; }} | tee .moor/{label}.log\n__moor_rc=$(cat .moor/{label}.rc)\nexit \"${{__moor_rc:-1}}\"\n"
    )
}
