//! Randomness adapters: `SystemRng` (OS entropy) and `FakeRng`.

use moor_core::seam::rng::Rng;

/// Real RNG backed by the operating system.
#[derive(Debug)]
pub(crate) struct SystemRng;

impl Rng for SystemRng {
    /// Falls back to a process-local counter mixed with the process id if the
    /// OS source fails; no clock is read.
    fn u32(&self) -> u32 {
        getrandom::u32().unwrap_or_else(|_| fallback_u32())
    }
}

/// Process-wide draw counter for the fallback.
static FALLBACK_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Distinct per call within a process: counter ^ pid, then an integer hash.
fn fallback_u32() -> u32 {
    let n = FALLBACK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut x = n.wrapping_add(1).wrapping_mul(0x9e37_79b1) ^ std::process::id();
    x ^= x >> 16;
    x = x.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 13;
    x
}

/// Deterministic RNG: a fixed, distinct sequence per draw.
#[cfg_attr(not(test), expect(dead_code, reason = "test fake"))]
#[derive(Debug, Default)]
pub(crate) struct FakeRng {
    /// Draws so far.
    n: std::sync::atomic::AtomicU32,
}

impl Rng for FakeRng {
    fn u32(&self) -> u32 {
        self.n
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_mul(0x0001_0001)
            .wrapping_add(0xab_cdef)
    }
}
