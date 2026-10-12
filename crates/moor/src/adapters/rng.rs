//! Randomness adapters: `SystemRng` (OS entropy) and `FakeRng`.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};

use moor_core::seam::rng::Rng;

/// Real RNG backed by the operating system.
#[derive(Debug)]
pub(crate) struct SystemRng;

impl Rng for SystemRng {
    /// Falls back to std's OS-seeded `RandomState` if the OS source fails.
    fn u32(&self) -> u32 {
        getrandom::u32().unwrap_or_else(|_| {
            let bits = RandomState::new().build_hasher().finish();
            u32::try_from(bits & 0xFFFF_FFFF).unwrap_or_default()
        })
    }
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
