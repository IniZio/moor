//! A schedule in words, and its next firings, so a person never has to read cron.

use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};

use moor_core::seam::clock::Ts;
use moor_core::workflow::WorkflowSchedule;
use serde_json::json;

use crate::login::account::json;

/// The days of the week as cron numbers them, from Sunday.
const DAYS: [&str; 7] = [
    "Sundays",
    "Mondays",
    "Tuesdays",
    "Wednesdays",
    "Thursdays",
    "Fridays",
    "Saturdays",
];

/// `cron` in words, such as "Weekdays at 9:00", when it is one of the shapes the form offers.
pub(super) fn words(cron: &str) -> Option<String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, "*", "*", day] = fields.as_slice() else {
        return None;
    };
    let minute: u8 = minute.parse().ok().filter(|m| *m < 60)?;
    if let Some(every) = hour.strip_prefix("*/") {
        let every: u8 = every.parse().ok().filter(|n| (2..24).contains(n))?;
        let at = if minute == 0 {
            String::new()
        } else {
            format!(" at :{minute:02}")
        };
        return (*day == "*").then(|| format!("Every {every} hours{at}"));
    }
    if *hour == "*" {
        let at = if minute == 0 {
            String::new()
        } else {
            format!(" at :{minute:02}")
        };
        return (*day == "*").then(|| format!("Every hour{at}"));
    }
    let hour: u8 = hour.parse().ok().filter(|h| *h < 24)?;
    let at = format!("at {hour}:{minute:02}");
    match *day {
        "*" => Some(format!("Every day {at}")),
        "1-5" | "MON-FRI" | "mon-fri" => Some(format!("Weekdays {at}")),
        d => {
            let n: usize = d.parse().ok().filter(|n| *n <= 7)?;
            DAYS.get(n % 7).map(|d| format!("{d} {at}"))
        }
    }
}

/// When `ms` falls in `zone`, with the zone named, such as "Fri 20 Nov, 08:30 (Asia/Hong_Kong)".
pub(super) fn at_in_zone(ms: i64, zone: &str) -> Option<String> {
    let tz = jiff::tz::TimeZone::get(zone).ok()?;
    let at = jiff::Timestamp::from_millisecond(ms).ok()?.to_zoned(tz);
    Some(format!("{} ({zone})", at.strftime("%a %-d %b, %H:%M")))
}

/// The next three firings of `cron` in `zone` after `now`, in Unix milliseconds, or why the
/// schedule is refused.
pub(super) fn next_three(cron: &str, zone: &str, now: Ts) -> Result<Vec<i64>, String> {
    let schedule = WorkflowSchedule::parse(cron, zone).map_err(|e| e.to_string())?;
    let mut at = now;
    let mut next = Vec::new();
    while next.len() < 3 {
        let Some(due) = schedule.first_after(at) else {
            break;
        };
        next.push(due.as_millis());
        at = due;
    }
    Ok(next)
}

/// Answers the words and next three firings of the query's `cron` in its `timezone`.
pub(crate) fn preview(stream: &mut TcpStream, query: &str) {
    let param = |key: &str| {
        oauth2::url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    let (cron, zone) = (param("cron"), param("timezone"));
    #[allow(
        clippy::disallowed_methods,
        reason = "a preview reads the wall clock once"
    )]
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    match next_three(&cron, &zone, Ts::from_millis(now)) {
        Ok(next) => json(
            stream,
            "200 OK",
            &json!({"words": words(&cron), "next": next}),
        ),
        Err(why) => json(stream, "400 Bad Request", &json!({"error": why})),
    }
}

#[cfg(test)]
mod tests {
    use moor_core::seam::clock::Ts;

    use super::{at_in_zone, next_three, words};

    #[test]
    fn a_firing_reads_in_its_own_zone_on_a_day_clock() {
        let monday_9_hkt = 1_763_946_000_000;
        assert_eq!(
            at_in_zone(monday_9_hkt, "Asia/Hong_Kong").as_deref(),
            Some("Mon 24 Nov, 09:00 (Asia/Hong_Kong)")
        );
        assert_eq!(
            at_in_zone(monday_9_hkt + 13 * 3_600_000, "UTC").as_deref(),
            Some("Mon 24 Nov, 14:00 (UTC)")
        );
        assert_eq!(at_in_zone(0, "Mars/Base"), None);
    }

    #[test]
    fn the_form_shapes_read_as_words() {
        for (cron, said) in [
            ("0 9 * * 1-5", "Weekdays at 9:00"),
            ("30 14 * * *", "Every day at 14:30"),
            ("0 8 * * 1", "Mondays at 8:00"),
            ("0 8 * * 0", "Sundays at 8:00"),
            ("0 */6 * * *", "Every 6 hours"),
            ("15 */2 * * *", "Every 2 hours at :15"),
            ("5 * * * *", "Every hour at :05"),
            ("0 * * * *", "Every hour"),
        ] {
            assert_eq!(words(cron).as_deref(), Some(said), "{cron}");
        }
    }

    #[test]
    fn any_other_cron_has_no_words() {
        for cron in [
            "0 9 1 * *",
            "0 9 * 1 *",
            "0 9,17 * * *",
            "0 9 * * 1,3",
            "bad",
            "0 */6 * * 1",
        ] {
            assert_eq!(words(cron), None, "{cron}");
        }
    }

    #[test]
    fn the_next_three_weekday_mornings_skip_the_weekend() {
        let friday_noon_utc = Ts::from_millis(1_763_726_400_000);
        let next = next_three("0 9 * * 1-5", "Asia/Hong_Kong", friday_noon_utc).unwrap();
        let monday_9_hkt = 1_763_946_000_000;
        assert_eq!(
            next,
            [
                monday_9_hkt,
                monday_9_hkt + 86_400_000,
                monday_9_hkt + 2 * 86_400_000
            ]
        );
    }

    #[test]
    fn a_schedule_the_controller_refuses_says_why() {
        let now = Ts::from_millis(0);
        assert!(next_three("*/5 * * * *", "UTC", now)
            .unwrap_err()
            .contains("fire less often"));
        assert!(next_three("0 9 * * *", "Mars/Base", now).is_err());
    }
}
