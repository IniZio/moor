//! Deleting the canonical cache volumes no Box has used for a long time.

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::{Caches, COPY_LABEL, KEY_LABEL};
use crate::boxes::docker::cli::Cli;

/// How long a canonical volume may sit unused before it goes, unless told otherwise.
pub(crate) const DEFAULT_MAX_AGE: Duration = Duration::from_hours(30 * 24);

/// What a pass decides for one canonical volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Expiry is off.
    Off,
    /// No use is recorded, so the clock starts now and nothing goes.
    Learn,
    /// Used within the age.
    Fresh,
    /// A Box, live or asleep, mounts it or holds a copy of it.
    Held,
    /// Unused for longer than the age and held by no Box.
    Expire,
}

/// What to do with a volume last used at `last`, seen at `now`, when it is `held` or not.
pub(super) fn verdict(last: Option<i64>, now: i64, max: Duration, held: bool) -> Verdict {
    if max.is_zero() {
        return Verdict::Off;
    }
    let Some(last) = last else {
        return Verdict::Learn;
    };
    let idle = now.saturating_sub(last);
    if idle < i64::try_from(max.as_secs()).unwrap_or(i64::MAX) {
        Verdict::Fresh
    } else if held {
        Verdict::Held
    } else {
        Verdict::Expire
    }
}

/// The log line for a deleted volume.
pub(super) fn line(name: &str, bytes: Option<u64>, idle_secs: i64) -> String {
    let size = bytes.map_or_else(|| "size unknown".to_owned(), |b| format!("{b} bytes"));
    format!("moor-machine-agent: deleted cache volume {name} ({size}), unused for {idle_secs}s")
}

/// Parses an age such as `30d`, `12h`, `90m`, `20s` or `0`; a bare number is days.
///
/// # Errors
/// Text that is not a number with an optional unit.
pub fn parse_age(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("{text:?} is not an age such as 30d or 20s"))?;
    let secs = match unit {
        "" | "d" => 24 * 3600,
        "h" => 3600,
        "m" => 60,
        "s" => 1,
        _ => return Err(format!("{text:?} is not an age such as 30d or 20s")),
    };
    Ok(Duration::from_secs(n.saturating_mul(secs)))
}

/// Whether a Box mounts `canonical`, running or asleep, or holds a private copy of it.
async fn held(canonical: &str) -> bool {
    let listed =
        |args: Vec<String>| async move { Cli::ok(args).await.map_or(true, |out| !out.is_empty()) };
    let mounted = vec![
        "ps".to_owned(),
        "-a".to_owned(),
        "-q".to_owned(),
        "--filter".to_owned(),
        format!("volume={canonical}"),
    ];
    let copied = vec![
        "volume".to_owned(),
        "ls".to_owned(),
        "-q".to_owned(),
        "--filter".to_owned(),
        format!("label={COPY_LABEL}={canonical}"),
    ];
    listed(mounted).await || listed(copied).await
}

/// The bytes `volume` holds, read with a throwaway container of `image`.
async fn size(image: &str, volume: &str) -> Option<u64> {
    let out = Cli::ok([
        "run",
        "--rm",
        "-u",
        "0",
        "--entrypoint",
        "du",
        "-v",
        &format!("{volume}:/v:ro"),
        image,
        "-sb",
        "/v",
    ])
    .await
    .ok()?;
    out.split_whitespace().next()?.parse().ok()
}

/// Deletes each canonical volume a pass finds expired. Only one pass runs at a time.
#[allow(clippy::print_stderr, reason = "the agent has no log but its stderr")]
pub(in crate::boxes::docker) async fn pass(caches: &Caches, image: &str, max: Duration) {
    if max.is_zero() || caches.sweeping.swap(true, Ordering::AcqRel) {
        return;
    }
    let listed = Cli::ok([
        "volume",
        "ls",
        "-q",
        "--filter",
        &format!("label={KEY_LABEL}"),
    ])
    .await
    .unwrap_or_default();
    for name in listed.lines().filter(|n| n.starts_with("moor-cache-")) {
        let lock = caches.of(name);
        let _write = lock.write().await;
        let now = crate::unix_now();
        let last = caches.uses.last(name);
        match verdict(last, now, max, held(name).await) {
            Verdict::Learn => caches.uses.touch_at([name], now),
            Verdict::Expire => {
                let bytes = size(image, name).await;
                if Cli::ok(["volume", "rm", name]).await.is_ok() {
                    caches.uses.forget(name);
                    eprintln!(
                        "{}",
                        line(name, bytes, now.saturating_sub(last.unwrap_or(now)))
                    );
                }
            }
            Verdict::Off | Verdict::Fresh | Verdict::Held => {}
        }
    }
    caches.sweeping.store(false, Ordering::Release);
}
