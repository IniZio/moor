//! Duration parsing for the user config.

use std::time::Duration;

/// `text` as a duration, or `default` when unset or unparsable.
pub(super) fn duration_or(text: Option<&str>, default: Duration) -> Duration {
    text.and_then(|t| parse_duration(t).ok()).unwrap_or(default)
}

/// Parses `<positive integer><s|m|h|d>`, e.g. `45m`.
pub(super) fn parse_duration(text: &str) -> Result<Duration, &'static str> {
    const MSG: &str = "must be a positive duration like 30s, 45m, 2h or 1d";
    let text = text.trim();
    let unit = text.chars().next_back().ok_or(MSG)?;
    let secs_per = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        _ => return Err(MSG),
    };
    let digits = text.strip_suffix(unit).ok_or(MSG)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(MSG);
    }
    let n: u64 = digits.parse().map_err(|_| "duration is too large")?;
    if n == 0 {
        return Err("must be greater than zero");
    }
    n.checked_mul(secs_per)
        .map(Duration::from_secs)
        .ok_or("duration is too large")
}
