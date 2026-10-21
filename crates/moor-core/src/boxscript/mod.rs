//! Shell script text run on a Box. Pure: builds text, never runs it.

pub mod boxwork;
pub mod record;
pub mod setup;
pub mod teardown;

#[cfg(test)]
mod tests;

/// POSIX single-quotes `s`: wraps it in `'...'` and renders each `'` as `'\''`. Always quotes.
#[must_use]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Runs its first argument under bash when the Box has bash, else under sh.
const BASH_OR_SH: &str =
    "if command -v bash >/dev/null 2>&1; then exec bash -c \"$1\"; fi; exec sh -c \"$1\"";

/// The argv that runs shell text `script` on a Box, under bash when the Box has it, else sh.
#[must_use]
pub fn shell_argv(script: &str) -> Vec<String> {
    ["sh", "-c", BASH_OR_SH, "sh", script]
        .map(str::to_owned)
        .to_vec()
}

/// Shell text that runs `argv` on a Box.
///
/// One element is a shell string and runs as `sh -c '<element>'`. Several elements are each
/// quoted and joined by spaces. An empty argv gives an empty string.
#[must_use]
pub fn command_line(argv: &[String]) -> String {
    match argv {
        [] => String::new(),
        [script] => format!("sh -c {}", quote(script)),
        _ => argv
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    }
}
