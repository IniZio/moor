//! Each token's tunnel rate: a bucket of `burst` tunnels that refills at `rate` a second.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use moor_core::seam::clock::Clock;

/// Buckets kept before full ones are forgotten, so unknown tokens cannot grow the map forever.
const KEPT: usize = 4096;

/// One token's bucket.
struct Bucket {
    /// Tunnels it may still open.
    left: f64,
    /// When `left` was last topped up.
    at: Instant,
}

/// The rate of every token this replica has seen, keyed by the token's hash.
pub(super) struct Limit {
    /// Tunnels a second.
    rate: f64,
    /// Bucket size.
    burst: f64,
    /// Source of time.
    clock: Arc<dyn Clock>,
    /// Each token's bucket.
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl Limit {
    /// Buckets of `burst` that refill at `rate` a second.
    pub(super) fn new(rate: f64, burst: f64, clock: Arc<dyn Clock>) -> Self {
        Self {
            rate,
            burst,
            clock,
            buckets: Mutex::default(),
        }
    }

    /// Takes one tunnel from the bucket of `token_hash`; false when it is empty.
    pub(super) fn allow(&self, token_hash: &str) -> bool {
        let now = self.clock.instant();
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() >= KEPT {
            let (rate, burst) = (self.rate, self.burst);
            buckets.retain(|_, b| topped(b, now, rate, burst) < burst);
        }
        let bucket = buckets.entry(token_hash.to_owned()).or_insert(Bucket {
            left: self.burst,
            at: now,
        });
        bucket.left = topped(bucket, now, self.rate, self.burst);
        bucket.at = now;
        let allowed = bucket.left >= 1.0;
        if allowed {
            bucket.left -= 1.0;
        }
        drop(buckets);
        allowed
    }
}

/// What `bucket` holds at `now`.
fn topped(bucket: &Bucket, now: Instant, rate: f64, burst: f64) -> f64 {
    let gained = now.saturating_duration_since(bucket.at).as_secs_f64() * rate;
    (bucket.left + gained).min(burst)
}
