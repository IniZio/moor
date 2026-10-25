//! The short line that says what an agent is doing now.

/// Most characters of a `doing` line; the rest is cut.
pub const DOING_MAX: usize = 80;

/// `raw` as a `doing` line: its first non-blank line, whitespace squeezed, cut to
/// [`DOING_MAX`] characters with `...` when cut; none when nothing is left.
///
/// Whoever builds a line decides what it may name; this only bounds its shape, so a line
/// can never carry a second line of output or an unbounded argument.
#[must_use]
pub fn doing_line(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if squeezed.chars().count() <= DOING_MAX {
        return Some(squeezed);
    }
    let kept: String = squeezed.chars().take(DOING_MAX - 3).collect();
    Some(format!("{}...", kept.trim_end()))
}

#[cfg(test)]
mod tests;
