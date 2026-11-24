//! What the egress tests read of the Boxes a replica serves.

use super::{lock, Boxes};

impl Boxes {
    /// How many tokens are kept.
    pub(in crate::proxy::egress) fn tracked(&self) -> usize {
        lock(&self.known).len()
    }

    /// Where each running engine takes its tunnels.
    pub(in crate::proxy::egress) fn engines(&self) -> Vec<std::net::SocketAddr> {
        lock(&self.served)
            .values()
            .filter_map(|slot| lock(&slot.held).as_ref().map(|h| h.served.addr))
            .collect()
    }

    /// How many Boxes have a running proxy.
    pub(in crate::proxy::egress) fn serving(&self) -> usize {
        lock(&self.served)
            .values()
            .filter(|slot| {
                lock(&slot.held)
                    .as_ref()
                    .is_some_and(|h| h.served.running())
            })
            .count()
    }
}
