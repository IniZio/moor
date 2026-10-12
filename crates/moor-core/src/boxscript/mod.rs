//! Shell script text run on a Box. Pure: builds text, never runs it.

pub mod record;
pub mod setup;

#[cfg(test)]
mod tests;

/// POSIX single-quotes `s`: wraps it in `'...'` and renders each `'` as `'\''`. Always quotes.
#[must_use]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
