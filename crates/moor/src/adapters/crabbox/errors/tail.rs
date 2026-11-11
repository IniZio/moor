//! Folds a stderr into the one line a summary carries.

/// Longest stderr tail, in characters, appended to a summary.
const TAIL_MAX: usize = 300;
/// Longest cause line, in characters, kept ahead of cleanup chatter in a long stderr.
const CAUSE_MAX: usize = 150;
/// Starts of the lines crabbox prints while it cleans up, after the cause of a failure.
const CHATTER: [&str; 2] = ["warn:", "cache group"];

/// `stderr` as one line of at most `TAIL_MAX` characters that keeps the last line not cleanup chatter.
pub(in crate::adapters::crabbox) fn one_line(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let joined = lines.join("; ");
    if joined.chars().count() <= TAIL_MAX {
        return joined;
    }
    let Some(at) = lines.iter().rposition(|l| !is_chatter(l)) else {
        return keep_end(&joined, TAIL_MAX);
    };
    let (Some(cause), Some(before), Some(after)) =
        (lines.get(at), lines.get(..at), lines.get(at + 1..))
    else {
        return keep_end(&joined, TAIL_MAX);
    };
    let cause_max = if after.is_empty() {
        TAIL_MAX
    } else {
        CAUSE_MAX
    };
    let mut out = keep_end(cause, cause_max);
    if !after.is_empty() {
        let room = TAIL_MAX - out.chars().count() - "; ".len();
        out = format!("{out}; {}", keep_end(&after.join("; "), room));
    }
    let room = TAIL_MAX.saturating_sub(out.chars().count() + "; ".len());
    if !before.is_empty() && room > 0 {
        out = format!("{}; {out}", keep_end(&before.join("; "), room));
    }
    out
}

/// Whether `line` is crabbox cleaning up rather than saying what failed.
fn is_chatter(line: &str) -> bool {
    let lower = line.to_lowercase();
    CHATTER.iter().any(|c| lower.starts_with(c))
}

/// `text`, cut from the front to at most `max` characters with a leading `…`.
fn keep_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - (max - 1)).collect();
    format!("…{}", tail.trim_start())
}
