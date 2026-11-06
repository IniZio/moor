//! Readying every awake Box ahead of its first request, so any replica serves it while the
//! controller is away.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::user::SecretName;
use moor_core::BoxId;

use super::{lock, Boxes, Bucket, Known};

/// How a replica keeps every awake Box ready to serve while the controller is away.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Warm {
    /// Pause between passes once synced.
    pub(crate) every: Duration,
    /// A pass that takes longer does not count, so a slow ledger never makes a replica Ready.
    pub(crate) deadline: Duration,
}

/// Warms `boxes` until `on` clears: a pass every `tick` until one finishes within the deadline,
/// then one every `warm.every`. Runs beside tending, so a slow ledger never holds up audit
/// delivery or the closing of Boxes' proxies.
pub(in crate::proxy::egress) fn keep_warm(
    boxes: &Boxes,
    warm: Warm,
    (on, clock, tick): (&AtomicBool, &dyn Clock, Duration),
) {
    while on.load(Ordering::Acquire) {
        let started = clock.instant();
        match boxes.warm() {
            Ok(()) if clock.instant().saturating_duration_since(started) < warm.deadline => {
                boxes.mark_synced();
            }
            Ok(()) => boxes.say(format_args!(
                "learning the awake Boxes took over {}s, so it does not count",
                warm.deadline.as_secs()
            )),
            Err(e) => boxes.say(format_args!("cannot learn the awake Boxes: {e}")),
        }
        let pause = if boxes.synced() { warm.every } else { tick };
        let mut slept = Duration::ZERO;
        while slept < pause && on.load(Ordering::Acquire) {
            clock.sleep(tick);
            slept += tick;
        }
    }
}

impl Boxes {
    /// Learns every awake open Box's token from the ledger, then starts each Box's proxy and
    /// reads the secrets its rules name and what it holds, so this replica serves any such Box
    /// on last answers while the controller is away, not only the Boxes it has already seen.
    ///
    /// # Errors
    /// The first read that failed; a pass with any failure leaves the replica unsynced.
    pub(in crate::proxy::egress) fn warm(&self) -> Result<(), String> {
        let awake = self.lookup.awake_tokens().map_err(|e| e.to_string())?;
        for (box_id, why) in &awake.unread {
            self.say(format_args!("Box {} is left cold: {why}", box_id.get()));
        }
        let tokens = awake.tokens;
        let now = self.clock.instant();
        let mut known = lock(&self.known);
        known.retain(|hash, _| tokens.iter().any(|(h, _)| h == hash));
        for (hash, box_id) in &tokens {
            known
                .entry(hash.clone())
                .or_insert_with(|| Known {
                    box_id: *box_id,
                    bucket: Bucket::new(self.token_rate.burst, now),
                    open: Arc::default(),
                })
                .box_id = *box_id;
        }
        drop(known);
        tokens.iter().try_for_each(|(_, box_id)| {
            self.warm_box(*box_id)
                .map_err(|e| format!("cannot warm Box {}: {e}", box_id.get()))
        })
    }

    /// Starts the proxy of `box_id` and reads each secret its rules name and what it holds.
    fn warm_box(&self, box_id: BoxId) -> Result<(), String> {
        let served = self.served(box_id)?;
        for rule in &served.spec.rules {
            let name = SecretName::parse(&rule.credential)?;
            served.ledger.secret(&name).map_err(|e| e.to_string())?;
        }
        served.ledger.hold().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Records that a [`Boxes::warm`] pass finished in time; the replica turns Ready.
    pub(in crate::proxy::egress) fn mark_synced(&self) {
        self.synced.store(true, Ordering::Release);
    }

    /// Whether the ledger has answered a [`Boxes::warm`] since this replica started; it stays
    /// true while the controller is away.
    pub(in crate::proxy::egress) fn synced(&self) -> bool {
        self.synced.load(Ordering::Acquire)
    }
}
