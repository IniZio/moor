//! Deterministic RNG fake.

use moor_core::seam::rng::Rng;

/// Deterministic RNG: a fixed, distinct sequence per draw.
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
