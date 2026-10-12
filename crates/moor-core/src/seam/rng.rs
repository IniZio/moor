//! Rng seam: the single injectable source of randomness.

/// Source of random numbers.
pub trait Rng: Send + Sync {
    /// A uniformly random `u32`.
    fn u32(&self) -> u32;
}
