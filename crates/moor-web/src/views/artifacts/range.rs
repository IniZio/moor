//! The `Range` request header, reduced to the one byte range moor-web serves.

/// What a `Range` header asks of a file.
pub(super) enum Wanted {
    /// The whole file: no header, a unit other than bytes, or several ranges.
    Whole,
    /// Bytes `start` to `end`, both inclusive and inside the file.
    Part {
        /// First byte.
        start: u64,
        /// Last byte.
        end: u64,
    },
    /// A malformed bytes range, or one that starts past the end of the file.
    Unsatisfiable,
}

/// Reads `header` against a file of `size` bytes.
pub(super) fn wanted(header: Option<&str>, size: u64) -> Wanted {
    let Some(spec) = header.and_then(|h| {
        let h = h.trim();
        h.get(..6)
            .filter(|unit| unit.eq_ignore_ascii_case("bytes="))
            .and_then(|_| h.get(6..))
    }) else {
        return Wanted::Whole;
    };
    if spec.contains(',') {
        return Wanted::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Wanted::Unsatisfiable;
    };
    let last_byte = size.saturating_sub(1);
    let bounds = match (digits(first), digits(last)) {
        (Some(start), Some(end)) if start <= end => Some((start, end.min(last_byte))),
        (Some(start), None) if last.is_empty() => Some((start, last_byte)),
        (None, Some(n)) if first.is_empty() && n > 0 => Some((size.saturating_sub(n), last_byte)),
        _ => None,
    };
    match bounds {
        Some((start, end)) if start < size => Wanted::Part { start, end },
        _ => Wanted::Unsatisfiable,
    }
}

/// A run of ASCII digits as a number, saturating at `u64::MAX`; anything else, a sign
/// included, is `None`.
fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(s.parse().unwrap_or(u64::MAX))
}
