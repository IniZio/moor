//! Timestamps as people read them.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Unix `secs` as a UTC time such as `2026-10-30T12:00:00Z`.
pub(crate) fn utc(secs: i64) -> String {
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| secs.to_string())
}

#[cfg(test)]
mod tests {
    use super::utc;

    #[test]
    fn a_unix_second_reads_as_utc() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
    }
}
