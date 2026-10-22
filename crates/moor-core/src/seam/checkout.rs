//! Checkout seam: the reads a client makes in its caller's git checkout.

use std::path::Path;

/// The caller's git checkout, read with the host's git.
pub trait Checkout {
    /// The `origin` URL of the checkout at `dir`; `None` outside a checkout or without one.
    fn origin_url(&self, dir: &Path) -> Option<String>;
    /// Whether the checkout at `dir` has uncommitted changes or commits its upstream lacks.
    fn local_changes(&self, dir: &Path) -> bool;
}
